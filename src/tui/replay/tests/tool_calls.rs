use crate::agent::AgentEvent;
use crate::tools;
use crate::tui::{
    UiEvent,
    replay::{handle_agent_event, replay_messages},
    test_support::new_app,
};
use serde_json::json;

#[test]
fn replays_failed_tool_calls() {
    let mut app = new_app();
    let read = |id: &str, path: &str| json!({ "type": "tool_use", "id": id, "name": "read", "input": { "path": path } });
    let result = |id: &str, is_error: bool| json!({ "type": "tool_result", "tool_use_id": id, "content": "missing", "is_error": is_error });
    replay_messages(
        &mut app,
        &[
            json!({ "role": "assistant", "content": [read("1", "a.rs"), read("2", "b.rs"), read("3", "c.rs")] }),
            json!({ "role": "user", "content": [result("1", false), result("2", true), result("3", false)] }),
        ],
    );
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["read a.rs, b.rs", "read failed: missing", "read c.rs"]
    );
}

#[test]
fn shows_a_failed_tool_call_live_as_it_is_replayed() {
    let mut app = new_app();
    for (path, error) in [("a.rs", None), ("b.rs", Some("missing")), ("c.rs", None)] {
        let input = json!({ "path": path });
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolStart {
                name: "read".into(),
                summary: tools::summary("read", &input),
                diff: None,
            }),
            &mut app,
        );
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolDone {
                name: "read".into(),
                error: error.map(Into::into),
                note: None,
            }),
            &mut app,
        );
    }
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["read a.rs, b.rs", "read failed: missing", "read c.rs"]
    );
}

#[test]
fn replays_a_cancelled_tool_call_as_it_appeared_live() {
    let mut app = new_app();
    let read = |id: &str, path: &str| json!({ "type": "tool_use", "id": id, "name": "read", "input": { "path": path } });
    let result = |id: &str, content: &str, is_error: bool| json!({ "type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error });
    replay_messages(
        &mut app,
        &[
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [read("1", "a.rs"), read("2", "b.rs"), read("3", "c.rs")] }),
            json!({ "role": "user", "content": [result("1", "a", false), result("2", "cancelled by the user", true), result("3", "cancelled by the user", true)] }),
            json!({ "role": "assistant", "stop_reason": "aborted", "content": [] }),
        ],
    );
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(texts, ["hello", "read a.rs, b.rs", "cancelled"]);
}
