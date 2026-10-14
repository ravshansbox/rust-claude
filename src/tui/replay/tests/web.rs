use crate::agent::AgentEvent;
use crate::tools;
use crate::tui::{
    App, UiEvent,
    replay::{handle_agent_event, replay_messages},
    test_support::new_app,
};
use serde_json::json;

#[test]
fn shows_web_search_and_sources_live_and_replayed() {
    let input = json!({ "query": "rust release" });
    let mut live = new_app();
    let events = [
        AgentEvent::ToolStart {
            name: "web_search".into(),
            summary: tools::summary("web_search", &input),
            diff: None,
        },
        AgentEvent::ToolDone {
            name: "web_search".into(),
            error: None,
            note: Some("1 result".into()),
        },
        AgentEvent::Text("Rust 1.99.0".into()),
        AgentEvent::Text(" is the latest.".into()),
        AgentEvent::Text("\n\nSources:\n- Rust Versions: https://releases.rs/\n".into()),
    ];
    for event in events {
        handle_agent_event(UiEvent::Agent(event), &mut live);
    }
    let citation = json!({ "type": "web_search_result_location", "url": "https://releases.rs/", "title": "Rust Versions" });
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[json!({ "role": "assistant", "content": [
            { "type": "server_tool_use", "id": "s1", "name": "web_search", "input": input },
            { "type": "web_search_tool_result", "tool_use_id": "s1", "content": [{ "type": "web_search_result", "url": "https://releases.rs/", "title": "Rust Versions" }] },
            { "type": "text", "text": "Rust 1.99.0", "citations": [citation] },
            { "type": "text", "text": " is the latest." },
        ] })],
    );
    let texts = |app: &App| -> Vec<String> {
        app.messages[1..]
            .iter()
            .map(|message| message.text.clone())
            .collect()
    };
    assert_eq!(
        texts(&live),
        [
            "web_search rust release",
            "web_search: 1 result",
            "Rust 1.99.0 is the latest.\n\nSources:\n- Rust Versions: https://releases.rs/\n"
        ]
    );
    assert_eq!(texts(&replayed), texts(&live));
}

#[test]
fn replays_a_failed_web_search() {
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[json!({ "role": "assistant", "content": [
            { "type": "server_tool_use", "id": "s1", "name": "web_search", "input": { "query": "q" } },
            { "type": "web_search_tool_result", "tool_use_id": "s1", "content": { "type": "web_search_tool_result_error", "error_code": "too_many_requests" } },
        ] })],
    );
    let texts: Vec<&str> = replayed.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["web_search q", "web_search failed: too_many_requests"]
    );
}

fn fetch_blocks(id: &str, url: &str, title: &str) -> [serde_json::Value; 2] {
    [
        json!({ "type": "server_tool_use", "id": id, "name": "web_fetch", "input": { "url": url } }),
        json!({ "type": "web_fetch_tool_result", "tool_use_id": id, "content": {
            "type": "web_fetch_result",
            "url": url,
            "content": { "type": "document", "source": { "type": "text", "media_type": "text/plain", "data": "text" }, "title": title },
        } }),
    ]
}

fn cited(index: u64) -> serde_json::Value {
    json!({ "type": "text", "text": "Quoted.", "citations": [{ "type": "char_location", "cited_text": "text", "document_index": index, "document_title": "Title" }] })
}

#[test]
fn replays_web_fetches_with_the_pages_cited_counted_since_the_last_compaction() {
    let [first_call, first_result] = fetch_blocks("s1", "https://example.com", "Example Domain");
    let [second_call, second_result] = fetch_blocks("s2", "https://example.org", "Example Org");
    let [third_call, third_result] = fetch_blocks("s3", "https://example.net", "Example Net");
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[
            json!({ "role": "user", "content": "fetch" }),
            json!({ "role": "assistant", "content": [first_call, first_result, cited(0)] }),
            json!({ "role": "user", "content": "again" }),
            json!({ "role": "assistant", "content": [second_call, second_result, cited(1)] }),
            json!({ "role": "user", "content": "", "stop_reason": "compacted", "summary": "s" }),
            json!({ "role": "user", "content": "more" }),
            json!({ "role": "assistant", "content": [third_call, third_result, cited(0)] }),
        ],
    );
    let texts: Vec<&str> = replayed.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .filter(|text| text.starts_with("web_fetch") || text.starts_with("Quoted"))
        .collect();
    assert_eq!(
        texts,
        [
            "web_fetch https://example.com",
            "web_fetch: Example Domain",
            "Quoted.\n\nSources:\n- Example Domain: https://example.com\n",
            "web_fetch https://example.org",
            "web_fetch: Example Org",
            "Quoted.\n\nSources:\n- Example Org: https://example.org\n",
            "web_fetch https://example.net",
            "web_fetch: Example Net",
            "Quoted.\n\nSources:\n- Example Net: https://example.net\n",
        ]
    );
}

#[test]
fn replays_a_failed_web_fetch() {
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[json!({ "role": "assistant", "content": [
            { "type": "server_tool_use", "id": "s1", "name": "web_fetch", "input": { "url": "https://example.com" } },
            { "type": "web_fetch_tool_result", "tool_use_id": "s1", "content": { "type": "web_fetch_tool_result_error", "error_code": "url_not_accessible" } },
        ] })],
    );
    let texts: Vec<&str> = replayed.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "web_fetch https://example.com",
            "web_fetch failed: url_not_accessible"
        ]
    );
}
