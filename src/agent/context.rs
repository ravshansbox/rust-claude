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
    if let Some(last) = messages.last_mut() {
        if let Some(text) = last["content"].as_str().map(str::to_owned) {
            last["content"] = json!([{ "type": "text", "text": text }]);
        }
        // The API does not allow cache_control on thinking blocks.
        if let Some(block) = last["content"].as_array_mut().and_then(|blocks| {
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
    messages
}

pub(super) fn estimate_tokens(message: &Value) -> u64 {
    let characters: usize = match &message["content"] {
        Value::String(text) => text.chars().count(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block["type"].as_str().unwrap_or_default() {
                "text" => block["text"].as_str().unwrap_or_default().chars().count(),
                "thinking" => block["thinking"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .count(),
                "tool_use" => {
                    block["name"].as_str().unwrap_or_default().chars().count()
                        + block["input"].to_string().chars().count()
                }
                "tool_result" => block["content"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .count(),
                _ => 0,
            })
            .sum(),
        _ => 0,
    };
    (characters as u64).div_ceil(4)
}

pub(super) fn estimate_prompt_tokens(system: &[Value], tools: &Value) -> u64 {
    let characters: usize = system
        .iter()
        .map(|block| block["text"].as_str().unwrap_or_default().chars().count())
        .sum::<usize>()
        + tools.to_string().chars().count();
    (characters as u64).div_ceil(4)
}

pub(super) fn estimate_text_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

pub(super) fn scale_parts(parts: Vec<(&'static str, u64)>, total: u64) -> Vec<(&'static str, u64)> {
    let estimated: u64 = parts.iter().map(|(_, tokens)| tokens).sum();
    if estimated == 0 {
        return parts;
    }
    parts
        .into_iter()
        .map(|(name, tokens)| {
            let scaled = (tokens as f64 * total as f64 / estimated as f64).round();
            (name, scaled as u64)
        })
        .collect()
}

pub struct ContextUse {
    pub parts: Vec<(&'static str, u64)>,
    pub total: u64,
    pub window: u64,
}

pub(super) fn context_tokens(messages: &[Value], system_tokens: u64) -> u64 {
    let last_usage = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            if message["role"] != "assistant" || message.get("stop_reason").is_some() {
                return None;
            }
            let usage = Usage::from_json(&message["usage"]);
            let tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
            (tokens > 0).then_some((index, tokens))
        });
    let (start, usage_tokens) =
        last_usage.map_or((0, system_tokens), |(index, tokens)| (index + 1, tokens));
    usage_tokens + messages[start..].iter().map(estimate_tokens).sum::<u64>()
}

#[cfg(test)]
mod tests {
    use super::{
        active_messages, context_tokens, has_uncompacted, scale_parts, with_cache_breakpoint,
    };
    use serde_json::json;

    #[test]
    fn scales_context_parts_to_total() {
        assert_eq!(
            scale_parts(vec![("a", 10), ("b", 30)], 80),
            vec![("a", 20), ("b", 60)]
        );
        assert_eq!(scale_parts(vec![("a", 0)], 80), vec![("a", 0)]);
    }

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
    fn counts_context_from_last_usage_and_trailing_messages() {
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
        assert_eq!(context_tokens(&messages, 50), 137);
    }

    #[test]
    fn estimates_context_without_usage() {
        let messages = vec![json!({ "role": "user", "content": "12345" })];
        assert_eq!(context_tokens(&messages, 50), 52);
    }
}
