use serde_json::{Value, json};

use crate::agent::{
    AgentEvent,
    test_support::{self, MockApi, Reply, text_reply},
};

fn web_search_reply(result: &Value) -> Reply {
    Reply::Events(vec![
        test_support::message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {} } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"query\": \"rust " } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "release\"}" } }),
        test_support::block_stop(0),
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": result } }),
        test_support::block_stop(1),
        json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "text", "text": "" } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "citations_delta", "citation": { "type": "web_search_result_location", "url": "https://releases.rs/", "title": "Rust Versions", "encrypted_index": "abc", "cited_text": "Stable: 1.99.0" } } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "Rust 1.99.0 is the latest." } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "citations_delta", "citation": { "type": "web_search_result_location", "url": "https://blog.rust-lang.org/", "title": "Rust Blog", "encrypted_index": "def", "cited_text": "Rust 1.99.0" } } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "citations_delta", "citation": { "type": "web_search_result_location", "url": "https://releases.rs/", "title": "Rust Versions", "encrypted_index": "ghi", "cited_text": "1.99.0" } } }),
        test_support::block_stop(2),
        test_support::message_delta("end_turn"),
        test_support::message_stop(),
    ])
}

fn search_results() -> Value {
    json!([
        { "type": "web_search_result", "url": "https://releases.rs/", "title": "Rust Versions", "encrypted_content": "first" },
        { "type": "web_search_result", "url": "https://blog.rust-lang.org/", "title": "Rust Blog", "encrypted_content": "second" },
    ])
}

#[tokio::test]
async fn offers_web_search_to_the_model() {
    let api = MockApi::start(vec![text_reply("hello")]).await;
    let (_, result) = test_support::run_prompt(&api, "hi", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    let tools = requests[0]["tools"].as_array().unwrap();
    assert!(
        tools.contains(&json!({ "type": "web_search_20250305", "name": "web_search" })),
        "{tools:?}"
    );
}

#[tokio::test]
async fn shows_a_web_search_its_results_and_the_sources_cited() {
    let api = MockApi::start(vec![
        web_search_reply(&search_results()),
        text_reply("again"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut lines = Vec::new();
    let mut text = String::new();
    let result = agent
        .prompt("search", &[], |event| match event {
            AgentEvent::ToolStart { name, summary, .. } => {
                lines.push(format!("{name} {summary}"));
            }
            AgentEvent::ToolDone { name, error, note } => {
                lines.push(format!("{name} {error:?} {note:?}"));
            }
            AgentEvent::Text(delta) => text.push_str(&delta),
            _ => {}
        })
        .await;
    let next = agent.prompt("more", &[], |_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    next.unwrap();
    assert_eq!(
        lines,
        [
            "web_search rust release",
            "web_search None Some(\"2 results\")"
        ]
    );
    assert_eq!(
        text,
        "Rust 1.99.0 is the latest.\n\nSources:\n- Rust Versions: https://releases.rs/\n- Rust Blog: https://blog.rust-lang.org/\n"
    );
    let requests = api.requests().await;
    let resent = &requests[1]["messages"][1]["content"];
    assert_eq!(resent[0]["input"], json!({ "query": "rust release" }));
    assert_eq!(resent[1]["content"], search_results());
    let indexes: Vec<&Value> = resent[2]["citations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|citation| &citation["encrypted_index"])
        .collect();
    assert_eq!(indexes, ["abc", "def", "ghi"]);
}

#[tokio::test]
async fn reports_a_web_search_that_failed() {
    let api = MockApi::start(vec![web_search_reply(&json!({
        "type": "web_search_tool_result_error",
        "error_code": "too_many_requests",
    }))])
    .await;
    let mut errors = Vec::new();
    let (_, result) = test_support::run_prompt(&api, "search", |event| {
        if let AgentEvent::ToolDone {
            error: Some(error), ..
        } = event
        {
            errors.push(error);
        }
    })
    .await;
    result.unwrap();
    assert_eq!(errors, ["too_many_requests"]);
}
