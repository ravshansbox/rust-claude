use super::{
    active_messages, context_tokens, has_uncompacted, shorten_tool_results, with_cache_breakpoint,
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
    let messages = vec![prompt.clone(), reply, compaction, prompt.clone()];
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
