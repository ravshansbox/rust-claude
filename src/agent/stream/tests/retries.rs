use super::*;

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
async fn retries_an_overloaded_reply_whose_error_breaks_off() {
    let api = MockApi::start(vec![Reply::CutOffOverloaded, text_reply("hello")]).await;
    assert_eq!(reply_text(&api).await, "hello");
    assert_eq!(api.requests().await.len(), 2);
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
