use serde_json::json;
use std::time::Duration;

use crate::agent::{
    Agent, AgentEvent,
    http::http_client_with,
    test_support::{self, MockApi, Reply, stopped_reply, text_reply},
};

mod incomplete;

async fn reply_text(api: &MockApi) -> String {
    let mut text = String::new();
    let (_, result) = test_support::run_prompt(api, "hi", test_support::text(&mut text)).await;
    result.unwrap();
    text
}

async fn fail_then_ask_again(first: Reply) -> (String, Vec<serde_json::Value>, Agent) {
    let api = MockApi::start(vec![first, text_reply("hello")]).await;
    let mut agent = test_support::agent(&api);
    let failed = agent.prompt("hi", &[], |_| {}).await;
    let next = agent.prompt("again", &[], |_| {}).await;
    test_support::remove_session(&agent);
    next.unwrap();
    let message = failed.unwrap_err().to_string();
    (message, api.requests().await, agent)
}

#[tokio::test]
async fn retries_a_reply_that_stops_arriving() {
    let api = MockApi::start(vec![Reply::Stall, text_reply("hello")]).await;
    let http = http_client_with(Duration::from_millis(300)).unwrap();
    let mut agent = test_support::agent_with_http(&api, http);
    let mut text = String::new();
    let mut notices = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        agent.prompt("hi", &[], |event| match event {
            AgentEvent::Text(delta) => text.push_str(&delta),
            AgentEvent::Notice(notice) => notices.push(notice),
            _ => {}
        }),
    )
    .await;
    test_support::remove_session(&agent);
    assert!(result.expect("prompt hung").is_ok());
    assert_eq!(text, "hello");
    assert_eq!(api.requests().await.len(), 2);
    assert!(notices.iter().any(|notice| notice.contains("retrying in")));
}

#[tokio::test]
async fn puts_together_a_reply_that_arrives_a_few_bytes_at_a_time() {
    let api = MockApi::start(vec![
        Reply::Trickled(vec![
            test_support::message_start(),
            test_support::text_block_start(),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "héllo 👋 " } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "wörld" } }),
            test_support::block_stop(0),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "call", "name": "read", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"path\": \"ünï" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "cødé.txt\"}" } }),
            test_support::block_stop(1),
            test_support::message_delta("tool_use"),
            test_support::message_stop(),
        ]),
        text_reply("done"),
    ])
    .await;
    assert_eq!(reply_text(&api).await, "héllo 👋 wörlddone");
    let requests = api.requests().await;
    let reply = &requests[1]["messages"][1]["content"];
    assert_eq!(reply[0]["text"], "héllo 👋 wörld");
    assert_eq!(reply[1]["input"], json!({ "path": "ünïcødé.txt" }));
}

#[tokio::test]
async fn skips_pings_and_events_it_does_not_know() {
    let api = MockApi::start(vec![Reply::Events(vec![
        test_support::message_start(),
        json!({ "type": "ping" }),
        test_support::text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "hel" } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "future_delta", "text": "?" } }),
        json!({ "type": "future_event" }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "lo" } }),
        test_support::block_stop(0),
        test_support::message_delta("end_turn"),
        test_support::message_stop(),
    ])])
    .await;
    assert_eq!(reply_text(&api).await, "hello");
}

#[tokio::test]
async fn skips_frames_without_data_such_as_keep_alive_comments() {
    let api = MockApi::start(vec![Reply::Events(vec![
        json!(": keep-alive\n\n"),
        test_support::message_start(),
        test_support::text_block_start(),
        json!("event: ping\n\n"),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "hello" } }),
        test_support::block_stop(0),
        test_support::message_delta("end_turn"),
        test_support::message_stop(),
    ])])
    .await;
    assert_eq!(reply_text(&api).await, "hello");
}

#[tokio::test]
async fn retries_an_overloaded_reply_whose_error_breaks_off() {
    let api = MockApi::start(vec![Reply::CutOffOverloaded, text_reply("hello")]).await;
    assert_eq!(reply_text(&api).await, "hello");
    assert_eq!(api.requests().await.len(), 2);
}

#[tokio::test]
async fn says_why_the_api_could_not_be_reached() {
    let api = MockApi::start(vec![]).await;
    let mut agent = test_support::agent(&api);
    agent.api_base = "http://127.0.0.1:1".into();
    let mut notices = Vec::new();
    let result = agent
        .prompt("hi", &[], test_support::notices(&mut notices))
        .await;
    test_support::remove_session(&agent);
    let message = result.unwrap_err().to_string();
    assert!(message.to_lowercase().contains("refused"), "{message}");
    assert!(notices[0].to_lowercase().contains("refused"), "{notices:?}");
}

#[tokio::test]
async fn does_not_retry_a_request_the_api_rejected() {
    let api = MockApi::start(vec![
        Reply::BadRequest("max_tokens is too large".into()),
        text_reply("hello"),
    ])
    .await;
    let mut notices = Vec::new();
    let (_, result) =
        test_support::run_prompt(&api, "hi", test_support::notices(&mut notices)).await;
    let message = result.unwrap_err().to_string();
    assert!(message.contains("max_tokens is too large"), "{message}");
    assert!(
        !notices.iter().any(|notice| notice.contains("retrying in")),
        "{notices:?}"
    );
    assert_eq!(api.requests().await.len(), 1);
}

#[tokio::test]
async fn says_when_a_reply_stopped_because_the_context_window_filled_up() {
    let api = MockApi::start(vec![stopped_reply(
        "the start of",
        "model_context_window_exceeded",
    )])
    .await;
    let mut notices = Vec::new();
    let (_, result) =
        test_support::run_prompt(&api, "hi", test_support::notices(&mut notices)).await;
    result.unwrap();
    assert_eq!(
        notices,
        vec!["reply cut off: the context window is full".to_string()]
    );
}

#[tokio::test]
async fn counts_tokens_of_a_reply_that_failed_and_was_retried() {
    let overloaded = Reply::Events(vec![
        json!({ "type": "message_start", "message": { "usage": { "input_tokens": 500, "output_tokens": 1 } } }),
        json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
    ]);
    let api = MockApi::start(vec![overloaded, text_reply("hello")]).await;
    let (agent, result) = test_support::run_prompt(&api, "hi", |_| {}).await;
    result.unwrap();
    assert_eq!(api.requests().await.len(), 2);
    let usage = agent.stats().usage;
    assert_eq!((usage.input, usage.output), (501, 2));
}

#[tokio::test]
async fn does_not_retry_an_overloaded_reply_after_text_arrived() {
    let overloaded = Reply::Events(vec![
        test_support::message_start(),
        test_support::text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial" } }),
        json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
    ]);
    let api = MockApi::start(vec![overloaded, text_reply("hello")]).await;
    let (_, result) = test_support::run_prompt(&api, "hi", |_| {}).await;
    let message = result.unwrap_err().to_string();
    assert!(message.contains("Overloaded"), "{message}");
    assert_eq!(api.requests().await.len(), 1);
}

#[tokio::test]
async fn does_not_retry_a_reply_cut_off_after_text_arrived() {
    let cut_off = Reply::CutOff(vec![
        test_support::message_start(),
        test_support::text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial" } }),
    ]);
    let api = MockApi::start(vec![cut_off, text_reply("hello")]).await;
    let mut text = String::new();
    let (_, result) = test_support::run_prompt(&api, "hi", test_support::text(&mut text)).await;
    assert!(result.is_err());
    assert_eq!(text, "partial");
    assert_eq!(api.requests().await.len(), 1);
}

#[tokio::test]
async fn keeps_tokens_of_a_failed_attempt_out_of_the_context_size() {
    let overloaded = Reply::Events(vec![
        json!({ "type": "message_start", "message": { "usage": { "input_tokens": 500, "cache_read_input_tokens": 500, "output_tokens": 1 } } }),
        json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
    ]);
    let api = MockApi::start(vec![overloaded, text_reply("hello")]).await;
    let (agent, result) = test_support::run_prompt(&api, "hi", |_| {}).await;
    result.unwrap();
    let stats = agent.stats();
    assert_eq!(stats.context_tokens, Some(2));
    assert_eq!(stats.usage.cache_read, 500);
}

#[tokio::test]
async fn continues_a_reply_the_api_paused() {
    let api = MockApi::start(vec![
        stopped_reply("searching", "pause_turn"),
        text_reply(" done"),
    ])
    .await;
    let mut text = String::new();
    let (_, result) = test_support::run_prompt(&api, "search", test_support::text(&mut text)).await;
    result.unwrap();
    assert_eq!(text, "searching done");
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "assistant");
    assert!(last.to_string().contains("searching"), "{last}");
}

#[tokio::test]
async fn shows_thinking_as_it_streams_and_sends_it_back_signed() {
    let api = MockApi::start(vec![
        Reply::Events(vec![
            test_support::message_start(),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Let me " } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "check." } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig" } }),
            test_support::block_stop(0),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Done." } }),
            test_support::block_stop(1),
            test_support::message_delta("end_turn"),
            test_support::message_stop(),
        ]),
        text_reply("again"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut thinking = Vec::new();
    let result = agent
        .prompt("hi", &[], |event| {
            if let AgentEvent::Thinking(delta) = event {
                thinking.push(delta);
            }
        })
        .await;
    let next = agent.prompt("more", &[], |_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    next.unwrap();
    assert_eq!(thinking, ["Let me ", "check."]);
    let requests = api.requests().await;
    assert_eq!(
        requests[1]["messages"][1]["content"][0],
        json!({ "type": "thinking", "thinking": "Let me check.", "signature": "sig" })
    );
}
