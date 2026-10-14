use super::{Activity, App, Picker, PickerKind, Role, time_ago, worker::UiEvent};
use crate::{
    agent::{AgentEvent, CANCELLED, parse_shell_message},
    session::SessionSummary,
    skills, tools,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

pub(super) fn handle_agent_event(event: UiEvent, app: &mut App) {
    match event {
        UiEvent::Agent(AgentEvent::Text(text)) => append_reply(app, &text),
        UiEvent::Agent(AgentEvent::Thinking(text)) => match app.messages.last_mut() {
            Some(last) if matches!(last.role, Role::Thinking) => last.append(&text),
            _ => app.push(Role::Thinking, text),
        },
        UiEvent::Agent(AgentEvent::ToolStart {
            name,
            summary,
            diff,
        }) => {
            if name == tools::agent::NAME {
                app.tasks.clear();
            }
            app.push_tool(&name, summary, diff.as_deref());
        }
        UiEvent::Agent(AgentEvent::ToolDone { name, error, note }) => {
            if name == "bash" {
                app.workspace_stale = true;
            }
            if let Some(error) = error {
                push_tool_failure(app, &name, &error);
            }
            if let Some(note) = note {
                push_tool_note(app, &name, &note);
            }
        }
        UiEvent::Agent(AgentEvent::Notice(text)) => app.push(Role::Event, text),
        UiEvent::Agent(AgentEvent::TaskStart { task, description }) => {
            app.start_task(task, &description);
        }
        UiEvent::Agent(AgentEvent::TaskToolCalls { task, count }) => {
            app.count_task_tool_calls(task, count);
        }
        UiEvent::Agent(AgentEvent::TaskStats { task, stats }) => app.show_task_stats(task, stats),
        UiEvent::Agent(AgentEvent::TaskDone {
            task,
            reply,
            tool_calls,
        }) => {
            app.finish_task(task, tool_calls, reply.err().as_deref());
        }
        UiEvent::McpRestarting {
            name,
            scope,
            notice,
        } => {
            app.push(Role::Event, notice);
            app.start_mcp_servers([(name.as_str(), scope)]);
        }
        UiEvent::McpServer(mut added) => {
            let diagnostics = std::mem::take(&mut added.diagnostics);
            app.finish_mcp_server(added);
            for diagnostic in diagnostics {
                app.push(Role::Event, diagnostic);
            }
        }
        UiEvent::Update(progress) => app.show_update(&progress),
        UiEvent::Agent(AgentEvent::Queued(prompt)) => app.push(Role::User, prompt),
        UiEvent::Agent(AgentEvent::Stats(stats)) => app.stats = stats,
        UiEvent::Done(result) => finish_reply(app, result),
        UiEvent::Cancelled(result) => {
            app.tasks.clear();
            app.push(Role::Event, "cancelled");
            app.restore_queued();
            app.finish(result, |_, ()| {});
        }
        UiEvent::Sessions(result) => app.finish(result, open_session_picker),
        UiEvent::NewSession(result) => app.finish(result, |app, ()| {
            app.clear_session();
            app.push(Role::Event, "new session");
        }),
        UiEvent::Shell(command, result) => {
            app.workspace_stale = true;
            app.finish(result, |app, output| app.push_shell(&command, &output));
        }
        UiEvent::ImagePasted(Ok(Some(image))) => app.attach_image(image),
        UiEvent::ImagePasted(Ok(None)) => app.push(Role::Event, "no image in the clipboard"),
        UiEvent::ImagePasted(Err(error)) => {
            app.push(Role::Event, format!("failed to paste image: {error:#}"));
        }
        UiEvent::Files(generation, files) => app.set_files(generation, files),
        UiEvent::Workspace(label) => app.workspace = label,
        UiEvent::Background(message) => {
            app.start_background();
            app.push_background(&message);
        }
        UiEvent::Resumed(result) => app.finish(result, |app, messages| {
            app.clear_session();
            replay_messages(app, &messages);
            app.push(Role::Event, "resumed session");
        }),
    }
}

fn finish_reply(app: &mut App, result: Result<()>) {
    app.tasks.clear();
    // Esc may come just as the reply finishes; it still keeps the
    // queued prompts from being sent.
    let cancelled = app.activity == Some(Activity::Cancelling);
    if cancelled && result.is_ok() {
        app.push(Role::Event, "cancelled");
    }
    if cancelled || result.is_err() {
        app.restore_queued();
    }
    if app.notifications && !app.focused && !cancelled {
        app.notification = Some(match &result {
            Ok(()) => "Done".into(),
            Err(error) => {
                let error = format!("{error:#}");
                format!("Failed: {}", error.lines().next().unwrap_or_default())
            }
        });
    }
    app.finish(result, |_, ()| {});
}

fn open_session_picker(app: &mut App, sessions: Vec<SessionSummary>) {
    if sessions.is_empty() {
        app.push(Role::Event, "no session to resume");
        return;
    }
    app.open_picker(Picker {
        kind: PickerKind::Session,
        title: "Resume session",
        items: sessions
            .into_iter()
            .map(|session| {
                let preview = session.preview.lines().next().unwrap_or_default();
                let label = format!("{:>8}  {preview}", time_ago(session.modified));
                (session.id, label)
            })
            .collect(),
        selected: 0,
    });
}

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
    for message in messages {
        if message["stop_reason"] == "compacted" {
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
                ("assistant", "web_search_tool_result") => {
                    let name = tools::web_search::NAME;
                    match tools::web_search::outcome(&block["content"]) {
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
            && let Some(sources) = tools::web_search::sources(blocks)
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
    app.push(Role::Event, format!("{name} failed: {error}"));
}

fn push_tool_note(app: &mut App, name: &str, note: &str) {
    app.push(Role::Event, format!("{name}: {note}"));
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
    if app.push_background(text) {
        return;
    }
    if let Some((command, output)) = parse_shell_message(text) {
        app.push_shell(command, output);
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
