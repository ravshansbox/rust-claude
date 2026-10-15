use serde_json::{Value, json};

use crate::agent::{
    AgentEvent,
    test_support::{self, MockApi, Reply, text_reply},
};

fn web_fetch_reply(url: &str, result: &Value, cited: u64) -> Reply {
    Reply::Events(vec![
        test_support::message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_fetch", "input": {} } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": format!("{{\"url\": \"{url}\"}}") } }),
        test_support::block_stop(0),
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "web_fetch_tool_result", "tool_use_id": "srvtoolu_1", "content": result } }),
        test_support::block_stop(1),
        json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "text", "text": "" } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "citations_delta", "citation": { "type": "char_location", "cited_text": "text", "document_index": cited, "document_title": "Title", "start_char_index": 0, "end_char_index": 4 } } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "Quoted." } }),
        test_support::block_stop(2),
        test_support::message_delta("end_turn"),
        test_support::message_stop(),
    ])
}

fn fetched_page(url: &str, title: &str) -> Value {
    json!({
        "type": "web_fetch_result",
        "url": url,
        "retrieved_at": "2026-10-14T12:00:00Z",
        "content": {
            "type": "document",
            "source": { "type": "text", "media_type": "text/plain", "data": "text" },
            "title": title,
            "citations": { "enabled": true },
        },
    })
}

#[tokio::test]
async fn offers_web_fetch_with_citations_to_the_model() {
    let api = MockApi::start(vec![text_reply("hello")]).await;
    let (_, result) = test_support::run_prompt(&api, "hi", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    let tools = requests[0]["tools"].as_array().unwrap();
    assert!(
        tools.contains(&json!({
            "type": "web_fetch_20250910",
            "name": "web_fetch",
            "citations": { "enabled": true },
        })),
        "{tools:?}"
    );
}

#[tokio::test]
async fn shows_a_web_fetch_and_the_page_cited_counting_earlier_fetches() {
    let api = MockApi::start(vec![
        web_fetch_reply(
            "https://example.com",
            &fetched_page("https://example.com", "Example Domain"),
            0,
        ),
        web_fetch_reply(
            "https://example.org/docs",
            &fetched_page("https://example.org/docs", "Example Docs"),
            1,
        ),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut lines = Vec::new();
    let mut text = String::new();
    let first = agent.prompt("fetch", &[], |_| {}).await;
    let second = agent
        .prompt("fetch another", &[], |event| match event {
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
    test_support::remove_session(&agent);
    first.unwrap();
    second.unwrap();
    assert_eq!(
        lines,
        [
            "web_fetch https://example.org/docs",
            "web_fetch None Some(\"Example Docs\")"
        ]
    );
    assert_eq!(
        text,
        "Quoted.\n\nSources:\n- Example Docs: https://example.org/docs\n"
    );
}

#[tokio::test]
async fn lists_a_cited_page_without_a_title_by_its_url() {
    let api = MockApi::start(vec![web_fetch_reply(
        "https://example.com",
        &fetched_page("https://example.com", ""),
        0,
    )])
    .await;
    let mut text = String::new();
    let (_, result) = test_support::run_prompt(&api, "fetch", |event| {
        if let AgentEvent::Text(delta) = event {
            text.push_str(&delta);
        }
    })
    .await;
    result.unwrap();
    assert_eq!(
        text,
        "Quoted.\n\nSources:\n- https://example.com: https://example.com\n"
    );
}

#[tokio::test]
async fn reports_a_web_fetch_that_failed() {
    let api = MockApi::start(vec![web_fetch_reply(
        "https://example.com",
        &json!({ "type": "web_fetch_tool_result_error", "error_code": "url_not_accessible" }),
        0,
    )])
    .await;
    let mut errors = Vec::new();
    let (_, result) = test_support::run_prompt(&api, "fetch", |event| {
        if let AgentEvent::ToolDone {
            error: Some(error), ..
        } = event
        {
            errors.push(error);
        }
    })
    .await;
    result.unwrap();
    assert_eq!(errors, ["url_not_accessible"]);
}
