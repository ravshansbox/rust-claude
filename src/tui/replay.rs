use super::{App, Role, tool_failure, tool_note};
use crate::{
    agent::{CANCELLED, parse_shell_message},
    skills, tools,
};
use serde_json::Value;
use std::collections::HashMap;

mod events;

pub(super) use events::handle_agent_event;

pub(super) fn replay_messages(app: &mut App, messages: &[Value]) {
    let results: HashMap<&str, (&Value, bool)> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| {
            Some((
                block["tool_use_id"].as_str()?,
                (&block["content"], block["is_error"] == true),
            ))
        })
        .collect();
    let mut context_start = 0;
    for (index, message) in messages.iter().enumerate() {
        if message["stop_reason"] == "compacted" {
            context_start = index + 1;
            app.push(Role::Event, "compacted conversation");
            continue;
        }
        if message["stop_reason"] == "aborted" {
            app.push(Role::Event, "cancelled");
            continue;
        }
        let mut cancelled = false;
        let role = message["role"].as_str().unwrap_or_default();
        if let Some(text) = message["content"].as_str() {
            replay_prompt(app, text);
            continue;
        }
        for block in message["content"].as_array().into_iter().flatten() {
            match (role, block["type"].as_str().unwrap_or_default()) {
                ("assistant", "text") => {
                    append_reply(app, block["text"].as_str().unwrap_or_default());
                }
                ("assistant", "thinking") => {
                    app.push(
                        Role::Thinking,
                        block["thinking"].as_str().unwrap_or_default(),
                    );
                }
                ("assistant", "tool_use") if !cancelled => {
                    cancelled = replay_tool_use(app, block, &results);
                }
                ("assistant", "server_tool_use") => {
                    let name = block["name"].as_str().unwrap_or_default();
                    app.push_tool(name, tools::summary(name, &block["input"]), None);
                }
                ("assistant", kind @ ("web_search_tool_result" | "web_fetch_tool_result")) => {
                    let (name, outcome) = if kind == "web_search_tool_result" {
                        let outcome = tools::web_search::outcome(&block["content"]);
                        (tools::web_search::NAME, outcome)
                    } else {
                        let outcome = tools::web_fetch::outcome(&block["content"]);
                        (tools::web_fetch::NAME, outcome)
                    };
                    match outcome {
                        Ok(note) => push_tool_note(app, name, &note),
                        Err(error) => push_tool_failure(app, name, &error),
                    }
                }
                ("user", "text") => replay_prompt(app, block["text"].as_str().unwrap_or_default()),
                _ => {}
            }
        }
        if role == "assistant"
            && let Some(blocks) = message["content"].as_array()
            && let Some(sources) = tools::sources::sources(&messages[context_start..=index], blocks)
        {
            append_reply(app, &sources);
        }
    }
}

fn replay_tool_use(app: &mut App, block: &Value, results: &HashMap<&str, (&Value, bool)>) -> bool {
    let name = block["name"].as_str().unwrap_or_default();
    app.push_tool(
        name,
        tools::summary(name, &block["input"]),
        tools::diff(name, &block["input"]).as_deref(),
    );
    match block["id"].as_str().and_then(|id| results.get(id)) {
        Some((error, true)) if *error == CANCELLED => return true,
        Some((error, true)) => {
            push_tool_failure(app, name, error.as_str().unwrap_or_default());
        }
        Some((Value::Array(replies), false)) if name == tools::agent::NAME => {
            let descriptions = block["input"]["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|task| task["description"].as_str().unwrap_or_default());
            let replies = replies
                .iter()
                .map(|reply| reply["text"].as_str().unwrap_or_default());
            app.push_finished_tasks(descriptions.zip(replies));
        }
        Some((result, false)) => {
            if let Some(note) = tools::note(name, result.as_str().unwrap_or_default()) {
                push_tool_note(app, name, &note);
            }
        }
        None => {}
    }
    false
}

fn push_tool_failure(app: &mut App, name: &str, error: &str) {
    app.push(Role::Event, tool_failure(name, error));
}

fn push_tool_note(app: &mut App, name: &str, note: &str) {
    app.push(Role::Event, tool_note(name, note));
}

fn append_reply(app: &mut App, text: &str) {
    match app.messages.last_mut() {
        Some(last) if matches!(last.role, Role::Assistant) => last.append(text),
        _ => {
            app.push(Role::Assistant, "");
            append_reply(app, text);
        }
    }
}

fn replay_prompt(app: &mut App, text: &str) {
    if let Some((command, output)) = parse_shell_message(text) {
        app.push_shell(command, true, output);
        app.add_prompt(format!("!{command}"));
        return;
    }
    if let Some(block) = skills::parse_block(text) {
        app.push(Role::Event, format!("[skill] {}", block.name));
        if let Some(user_message) = block.user_message {
            app.push(Role::User, user_message);
        }
        app.add_prompt(block.command());
    } else {
        app.push(Role::User, text);
        app.add_prompt(text.to_string());
    }
}

#[cfg(test)]
mod tests;
