use super::*;

#[tokio::test]
async fn discards_a_reply_the_model_declined_partway_through() {
    let refusal = Reply::Events(vec![
        json!({ "type": "message_start", "message": { "usage": { "input_tokens": 10, "output_tokens": 1 } } }),
        test_support::text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial answer" } }),
        test_support::block_stop(0),
        json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": { "output_tokens": 5 } }),
        test_support::message_stop(),
    ]);
    let (message, requests, agent) = fail_then_ask_again(refusal).await;
    assert!(message.contains("declined to answer"), "{message}");
    let resent = requests[1]["messages"].to_string();
    assert!(!resent.contains("partial answer"), "{resent}");
    assert!(resent.contains("again"), "{resent}");
    let usage = agent.stats().usage;
    assert_eq!((usage.input, usage.output), (11, 6));
}

#[tokio::test]
async fn says_the_model_declined_when_it_stops_partway_through_a_tool_call() {
    // The tool input may be whole or cut off where the model stopped.
    for partial in [
        "{\"command\": \"touch declined\"}",
        "{\"command\": \"touch declined",
    ] {
        let refusal = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 10, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "t1", "name": "bash", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": partial } }),
            test_support::block_stop(0),
            json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": { "output_tokens": 5 } }),
            test_support::message_stop(),
        ]);
        let (message, requests, agent) = fail_then_ask_again(refusal).await;
        assert!(
            message.contains("declined to answer"),
            "{partial}: {message}"
        );
        let resent = requests[1]["messages"].to_string();
        assert!(!resent.contains("touch declined"), "{partial}: {resent}");
        let usage = agent.stats().usage;
        assert_eq!((usage.input, usage.output), (11, 6), "{partial}");
    }
}

#[tokio::test]
async fn reports_a_reply_that_ends_before_it_finishes() {
    let cut_off = Reply::Events(vec![
        test_support::message_start(),
        test_support::text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "half an answer" } }),
    ]);
    let (message, requests, _) = fail_then_ask_again(cut_off).await;
    assert!(
        message.contains("response ended before the reply finished"),
        "{message}"
    );
    assert_eq!(requests.len(), 2);
    let resent = requests[1]["messages"].to_string();
    assert!(!resent.contains("half an answer"), "{resent}");
}

#[tokio::test]
async fn does_not_run_a_tool_call_cut_off_by_the_output_limit() {
    let cut_off = Reply::Events(vec![
        test_support::message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "t1", "name": "bash", "input": {} } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"command\": \"echo cut" } }),
        test_support::block_stop(0),
        test_support::message_delta("max_tokens"),
        test_support::message_stop(),
    ]);
    let api = MockApi::start(vec![cut_off]).await;
    let mut tools_started = 0;
    let (_, result) = test_support::run_prompt(&api, "hi", |event| {
        if let AgentEvent::ToolStart { .. } = event {
            tools_started += 1;
        }
    })
    .await;
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("reply stopped (max_tokens) before its tool call finished"),
        "{message}"
    );
    assert_eq!(tools_started, 0);
    assert_eq!(api.requests().await.len(), 1);
}

#[tokio::test]
async fn does_not_run_or_keep_a_tool_call_whose_input_is_broken() {
    let broken = Reply::Events(vec![
        test_support::message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "t1", "name": "bash", "input": {} } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"command\": \"echo broken" } }),
        test_support::block_stop(0),
        test_support::message_delta("tool_use"),
        test_support::message_stop(),
    ]);
    let api = MockApi::start(vec![broken, text_reply("ok")]).await;
    let mut agent = test_support::agent(&api);
    let mut tools_started = 0;
    let first = agent
        .prompt("hi", &[], |event| {
            if let AgentEvent::ToolStart { .. } = event {
                tools_started += 1;
            }
        })
        .await;
    let next = agent.prompt("next", &[], |_| {}).await;
    test_support::remove_session(&agent);
    assert!(first.is_err());
    next.unwrap();
    assert_eq!(tools_started, 0);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let after = requests[1]["messages"].to_string();
    assert!(!after.contains("echo broken"), "{after}");
    assert!(!after.contains("tool_use"), "{after}");
}
