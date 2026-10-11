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
    if content.chars().count() > limit {
        let kept: String = content.chars().take(limit).collect();
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
    if let Some(text) = message["content"].as_str().map(str::to_owned) {
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
        let usage = Usage::from_json(&message["usage"]);
        let tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
        (tokens > 0).then_some(tokens)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        active_messages, context_tokens, has_uncompacted, shorten_tool_results,
        with_cache_breakpoint,
    };
    use serde_json::json;

    #[test]
    fn replaces_compacted_messages_with_summary() {
        let prompt = json!({ "role": "user", "content": "hello" });
        let reply = json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] });
        let compaction = json!({
            "role": "assistant",
            "stop_reason": "compacted",
            "content": [],
            "summary": "greeted",
        });
        let messages = vec![prompt.clone(), reply, compaction.clone(), prompt.clone()];
        let active = active_messages(&messages);
        assert_eq!(active.len(), 2);
        assert!(active[0]["content"].as_str().unwrap().ends_with("greeted"));
        assert_eq!(active[1], prompt);
        assert!(has_uncompacted(&messages));
        assert!(!has_uncompacted(&messages[..3]));
        assert!(!has_uncompacted(&[]));
    }

    #[test]
    fn drops_empty_assistant_replies_from_requests() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [], "usage": { "output": 3 } }),
            json!({ "role": "user", "content": "again" }),
        ];
        let request = with_cache_breakpoint(&messages);
        assert_eq!(request.len(), 2);
        assert_eq!(request[1]["content"][0]["text"], "again");
    }

    #[test]
    fn marks_the_end_of_the_previous_request_after_a_cancelled_tool_round() {
        let messages = vec![
            json!({ "role": "user", "content": "run it" }),
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "1", "name": "bash", "input": {} }] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "1", "content": "cancelled by the user" }] }),
            json!({ "role": "assistant", "stop_reason": "aborted", "content": [] }),
            json!({ "role": "user", "content": "next" }),
        ];
        let request = with_cache_breakpoint(&messages);
        let marked: Vec<bool> = request
            .iter()
            .map(|message| message.to_string().contains("cache_control"))
            .collect();
        assert_eq!(marked, [true, false, false, true]);
    }

    #[test]
    fn keeps_the_cache_breakpoint_off_thinking_blocks() {
        let thinking = json!({ "type": "thinking", "thinking": "hmm", "signature": "s" });
        let redacted = json!({ "type": "redacted_thinking", "data": "d" });
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hi" }, thinking, redacted],
            }),
        ];
        let request = with_cache_breakpoint(&messages);
        assert_eq!(
            request[1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(request[1]["content"][1].get("cache_control"), None);
        assert_eq!(request[1]["content"][2].get("cache_control"), None);

        let only_thinking = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [thinking] }),
        ];
        let request = with_cache_breakpoint(&only_thinking);
        assert!(!request[1].to_string().contains("cache_control"));
    }

    #[test]
    fn counts_context_from_the_last_reply_only() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hi" }],
                "usage": { "input": 10, "output": 5, "cache_read": 100, "cache_write": 20 },
            }),
            json!({
                "role": "assistant",
                "stop_reason": "aborted",
                "content": [],
                "usage": { "input": 999 },
            }),
            json!({ "role": "user", "content": "12345678" }),
        ];
        assert_eq!(context_tokens(&messages), Some(135));
    }

    #[test]
    fn has_no_context_count_before_a_reply() {
        let messages = vec![json!({ "role": "user", "content": "12345" })];
        assert_eq!(context_tokens(&messages), None);
    }

    #[test]
    fn shortens_each_text_block_of_a_tool_result() {
        let messages = vec![json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "call-1",
                "content": [
                    { "type": "text", "text": "abcdef" },
                    { "type": "text", "text": "abc" },
                ],
            }],
        })];
        let shortened = shorten_tool_results(&messages, 3);
        let cut = "\n[the rest of this output was left out to fit the context window]";
        assert_eq!(
            shortened[0]["content"][0]["content"],
            json!([
                { "type": "text", "text": format!("abc{cut}") },
                { "type": "text", "text": "abc" },
            ])
        );
    }
}
