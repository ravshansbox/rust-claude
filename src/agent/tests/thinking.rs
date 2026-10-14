use serde_json::json;

use crate::agent::test_support::{self, MockApi, Reply, text_reply};

#[tokio::test]
async fn drops_thinking_bound_to_a_changed_conversation_for_the_rest_of_the_session() {
    let mismatch = "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation. Remove the block, or set `thinking.block_binding.prefix_mismatch_behavior` to \"drop_block\". The `tools` list differs from when the block was created.";
    let api = MockApi::start(vec![
        text_reply("first"),
        Reply::BadRequest(mismatch.into()),
        text_reply("second"),
        text_reply("third"),
        text_reply("after restart"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut notices = Vec::new();
    let first = agent.prompt("one", &[], |_| {}).await;
    let second = agent
        .prompt("two", &[], test_support::notices(&mut notices))
        .await;
    let third = agent.prompt("three", &[], |_| {}).await;
    // Close the session, as quitting does, so that it can be resumed.
    let id = agent.session.id.clone();
    drop(agent);
    let mut restarted = test_support::agent(&api);
    let resumed = restarted.resume(&id).await;
    let after_restart = restarted.prompt("four", &[], |_| {}).await;
    test_support::remove_session(&restarted);
    first.unwrap();
    second.unwrap();
    third.unwrap();
    resumed.unwrap();
    after_restart.unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    let requests = api.requests().await;
    let headers = api.headers().await;
    assert_eq!(requests.len(), 5);
    let drop_block = json!({ "prefix_mismatch_behavior": "drop_block" });
    for index in 0..2 {
        assert_eq!(requests[index]["thinking"].get("block_binding"), None);
        assert!(!headers[index].contains("thinking-binding-controls-2026-08-01"));
    }
    for index in 2..5 {
        assert_eq!(requests[index]["thinking"]["block_binding"], drop_block);
        assert!(
            headers[index]
                .contains("anthropic-beta: oauth-2025-04-20,thinking-binding-controls-2026-08-01"),
            "{}",
            headers[index]
        );
    }
}

#[tokio::test]
async fn turns_thinking_off_without_asking_to_drop_mismatched_thinking() {
    for (model, kind) in [
        ("claude-sonnet-5-5", "between_tools"),
        ("claude-haiku-5-5", "disabled"),
    ] {
        let api = MockApi::start(vec![text_reply("done")]).await;
        let mut agent = test_support::agent(&api);
        agent.model = model.into();
        agent.effort = "off";
        agent.session.drop_mismatched_thinking();
        let result = agent.prompt("hello", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        let requests = api.requests().await;
        let headers = api.headers().await;
        assert_eq!(requests[0]["thinking"], json!({ "type": kind }), "{model}");
        assert_eq!(requests[0].get("output_config"), None, "{model}");
        assert!(
            headers[0].contains("anthropic-beta: oauth-2025-04-20\r\n"),
            "{}",
            headers[0]
        );
    }
}
