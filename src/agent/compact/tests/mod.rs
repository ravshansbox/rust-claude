use serde_json::json;

use super::super::{
    Queued,
    test_support::{self, MockApi, Reply, full_context, stopped_reply, text_reply, tool_reply},
};

mod too_long;

/// Compacts between two prompts with `summary` as the summary reply, and
/// returns the compaction error and the messages sent with the next prompt.
async fn failed_compaction(summary: Reply) -> (String, String) {
    let api = MockApi::start(vec![text_reply("hello"), summary, text_reply("ok")]).await;
    let mut agent = test_support::agent(&api);
    let first = agent.prompt("hi", &[], |_| {}).await;
    let compacted = agent.compact(|_| {}).await;
    let next = agent.prompt("next", &[], |_| {}).await;
    test_support::remove_session(&agent);
    first.unwrap();
    next.unwrap();
    let requests = api.requests().await;
    (
        compacted.unwrap_err().to_string(),
        requests[2]["messages"].to_string(),
    )
}

#[tokio::test]
async fn keeps_the_conversation_when_the_summary_does_not_finish() {
    for stop_reason in ["max_tokens", "model_context_window_exceeded", "refusal"] {
        let (message, after) =
            failed_compaction(stopped_reply("partial summary", stop_reason)).await;
        assert!(
            message.contains("not compacted"),
            "{stop_reason}: {message}"
        );
        assert!(!after.contains("partial summary"), "{stop_reason}: {after}");
        assert!(after.contains("hello"), "{stop_reason}: {after}");
    }
}

#[tokio::test]
async fn keeps_the_conversation_when_the_summary_is_empty() {
    let (message, after) = failed_compaction(text_reply("  ")).await;
    assert_eq!(message, "compaction returned no summary");
    assert!(after.contains("hello"), "{after}");
}

#[tokio::test]
async fn says_there_is_nothing_to_compact_in_a_new_session() {
    let api = MockApi::start(Vec::new()).await;
    let mut agent = test_support::agent(&api);
    let compacted = agent.compact(|_| {}).await;
    test_support::remove_session(&agent);
    assert_eq!(compacted.unwrap_err().to_string(), "nothing to compact");
    assert_eq!(api.requests().await, Vec::<serde_json::Value>::new());
}

#[tokio::test]
async fn answers_the_prompt_when_automatic_compaction_fails() {
    let api = MockApi::start(vec![
        full_context(text_reply("hello")),
        stopped_reply("partial summary", "refusal"),
        text_reply("ok"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut notices = Vec::new();
    let first = agent.prompt("hi", &[], |_| {}).await;
    let next = agent
        .prompt("next", &[], test_support::notices(&mut notices))
        .await;
    test_support::remove_session(&agent);
    first.unwrap();
    next.unwrap();
    assert!(
        notices
            .iter()
            .any(|notice| notice.contains("not compacted")),
        "{notices:?}"
    );
    let requests = api.requests().await;
    let last = requests[2]["messages"].to_string();
    assert!(last.contains("hello"), "{last}");
    assert!(!last.contains("partial summary"), "{last}");
}

#[tokio::test]
async fn keeps_queued_prompts_word_for_word_when_compacting_after_a_tool_round() {
    let api = MockApi::start(vec![
        full_context(tool_reply("call-1", "bash", &json!({ "command": "true" }))),
        text_reply("summary of the work"),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    agent.queue.lock().unwrap().push(Queued {
        prompt: "also update the README".into(),
        images: Vec::new(),
    });
    let result = agent.prompt("fix the bug", &[], |_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let after_compaction = requests[2]["messages"].to_string();
    assert!(
        after_compaction.contains("summary of the work"),
        "{after_compaction}"
    );
    assert!(
        after_compaction.contains("also update the README"),
        "{after_compaction}"
    );
}
