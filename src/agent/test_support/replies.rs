use super::Reply;
use serde_json::{Value, json};

/// The first event of a reply, using one input and one output token.
pub(crate) fn message_start() -> Value {
    json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } })
}

/// The start of an empty text block at index 0.
pub(crate) fn text_block_start() -> Value {
    json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })
}

/// The closing usage of a reply that ends with `stop_reason`, using one output token.
pub(crate) fn message_delta(stop_reason: &str) -> Value {
    json!({ "type": "message_delta", "delta": { "stop_reason": stop_reason }, "usage": { "output_tokens": 1 } })
}

/// The last event of a reply.
pub(crate) fn message_stop() -> Value {
    json!({ "type": "message_stop" })
}

/// The end of the block at `index`.
pub(crate) fn block_stop(index: usize) -> Value {
    json!({ "type": "content_block_stop", "index": index })
}

pub(crate) fn text_reply(text: &str) -> Reply {
    stopped_reply(text, "end_turn")
}

/// A text reply that ends with `stop_reason`.
pub(crate) fn stopped_reply(text: &str, stop_reason: &str) -> Reply {
    Reply::Events(vec![
        message_start(),
        text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } }),
        block_stop(0),
        message_delta(stop_reason),
        message_stop(),
    ])
}

pub(crate) fn tool_reply(id: &str, name: &str, input: &Value) -> Reply {
    Reply::Events(vec![
        message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": id, "name": name, "input": input } }),
        block_stop(0),
        message_delta("tool_use"),
        message_stop(),
    ])
}

/// `reply`, reporting that the request filled the whole context window.
pub(crate) fn full_context(reply: Reply) -> Reply {
    let Reply::Events(mut events) = reply else {
        panic!("only event replies report usage");
    };
    events[0]["message"]["usage"]["input_tokens"] = json!(100_000_000);
    Reply::Events(events)
}
