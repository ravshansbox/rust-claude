use super::Usage;
use serde_json::{Value, json};

const SUMMARY_INTRODUCTION: &str = "The earlier conversation was compacted to save context. You wrote the summary below of everything that happened in it. Treat it as an accurate record and continue from where the conversation stopped.";

pub(super) fn is_compaction(message: &Value) -> bool {
    message["stop_reason"] == "compacted"
}

pub(super) fn active_messages(messages: &[Value]) -> Vec<Value> {
    let Some(index) = messages.iter().rposition(is_compaction) else {
        return messages.to_vec();
    };
    let summary = messages[index]["summary"].as_str().unwrap_or_default();
    let mut active = vec![json!({
        "role": "user",
        "content": format!("{SUMMARY_INTRODUCTION}\n\n{summary}"),
    })];
    active.extend_from_slice(&messages[index + 1..]);
    active
}

pub(super) fn has_uncompacted(messages: &[Value]) -> bool {
    let start = messages
        .iter()
        .rposition(is_compaction)
        .map_or(0, |index| index + 1);
    messages[start..]
        .iter()
        .any(|message| message.get("stop_reason").is_none())
}

/// Copies `messages` with each tool output cut to its first `limit`
/// characters. Tool calls and their results stay paired.
pub(super) fn shorten_tool_results(messages: &[Value], limit: usize) -> Vec<Value> {
    let mut messages = messages.to_vec();
    let blocks = messages
        .iter_mut()
        .filter_map(|message| message["content"].as_array_mut())
        .flatten();
    for block in blocks.filter(|block| block["type"] == "tool_result") {
        match &mut block["content"] {
            Value::Array(parts) => {
                for part in parts.iter_mut().filter(|part| part["type"] == "text") {
                    shorten(&mut part["text"], limit);
                }
            }
            content => shorten(content, limit),
        }
    }
    messages
}

fn shorten(text: &mut Value, limit: usize) {
    let Some(content) = text.as_str() else {
        return;
    };
    if let Some((end, _)) = content.char_indices().nth(limit) {
        let kept = &content[..end];
        *text = json!(format!(
            "{kept}\n[the rest of this output was left out to fit the context window]"
        ));
    }
}

pub(super) fn with_cache_breakpoint(messages: &[Value]) -> Vec<Value> {
    let mut messages: Vec<Value> = messages
        .iter()
        .filter(|message| {
            message.get("stop_reason").is_none()
                && message["content"]
                    .as_array()
                    .is_none_or(|blocks| !blocks.is_empty())
        })
        .cloned()
        .collect();
    for message in &mut messages {
        if let Some(object) = message.as_object_mut() {
            object.remove("usage");
        }
    }
    // Every request ends with a user message, so the last user message before
    // the last reply is where the previous request put its breakpoint and
    // wrote its cache entry. More than one user message can follow that reply,
    // such as tool results and a queued prompt. The API only looks 20 blocks
    // back from a breakpoint for an earlier entry, which a round with many
    // tool calls can exceed, so mark that message again. With the system
    // prompt's breakpoint this makes three of the four the API allows.
    if let Some((last, earlier)) = messages.split_last_mut() {
        mark_last_block(last);
        let reply = earlier
            .iter()
            .rposition(|message| message["role"] == "assistant")
            .unwrap_or(0);
        if let Some(previous) = earlier[..reply]
            .iter_mut()
            .rfind(|message| message["role"] == "user")
        {
            mark_last_block(previous);
        }
    }
    messages
}

fn mark_last_block(message: &mut Value) {
    if let Value::String(text) = &mut message["content"] {
        let text = std::mem::take(text);
        message["content"] = json!([{ "type": "text", "text": text }]);
    }
    // The API does not allow cache_control on thinking blocks.
    if let Some(block) = message["content"].as_array_mut().and_then(|blocks| {
        blocks.iter_mut().rfind(|block| {
            !matches!(
                block["type"].as_str(),
                Some("thinking" | "redacted_thinking")
            )
        })
    }) {
        block["cache_control"] = json!({ "type": "ephemeral" });
    }
}

pub(super) fn context_tokens(messages: &[Value]) -> Option<u64> {
    messages.iter().rev().find_map(|message| {
        if message["role"] != "assistant" || message.get("stop_reason").is_some() {
            return None;
        }
        let tokens = Usage::from_json(&message["usage"]).tokens();
        (tokens > 0).then_some(tokens)
    })
}

#[cfg(test)]
mod tests;
