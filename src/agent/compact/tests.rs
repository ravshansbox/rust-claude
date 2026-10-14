use serde_json::json;

use super::super::{
    Queued,
    test_support::{self, MockApi, full_context, stopped_reply, text_reply, tool_reply},
};

#[tokio::test]
async fn keeps_the_conversation_when_the_summary_does_not_finish() {
    for stop_reason in ["max_tokens", "model_context_window_exceeded", "refusal"] {
        let api = MockApi::start(vec![
            text_reply("hello"),
            stopped_reply("partial summary", stop_reason),
            text_reply("ok"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let first = agent.prompt("hi", &[], |_| {}).await;
        let compacted = agent.compact(|_| {}).await;
        let next = agent.prompt("next", &[], |_| {}).await;
        test_support::remove_session(&agent);
        first.unwrap();
        next.unwrap();
        let message = compacted.unwrap_err().to_string();
        assert!(
            message.contains("not compacted"),
            "{stop_reason}: {message}"
        );
        let requests = api.requests().await;
        let after = requests[2]["messages"].to_string();
        assert!(!after.contains("partial summary"), "{stop_reason}: {after}");
        assert!(after.contains("hello"), "{stop_reason}: {after}");
    }
}

#[tokio::test]
async fn keeps_the_conversation_when_the_summary_is_empty() {
    let api = MockApi::start(vec![
        text_reply("hello"),
        text_reply("  "),
        text_reply("ok"),
    ])
    .await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    let first = agent.prompt("hi", &[], |_| {}).await;
    let compacted = agent.compact(|_| {}).await;
    let next = agent.prompt("next", &[], |_| {}).await;
    test_support::remove_session(&agent);
    first.unwrap();
    next.unwrap();
    assert_eq!(
        compacted.unwrap_err().to_string(),
        "compaction returned no summary"
    );
    let requests = api.requests().await;
    let after = requests[2]["messages"].to_string();
    assert!(after.contains("hello"), "{after}");
}

#[tokio::test]
async fn says_there_is_nothing_to_compact_in_a_new_session() {
    let api = MockApi::start(Vec::new()).await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    let compacted = agent.compact(|_| {}).await;
    test_support::remove_session(&agent);
    assert_eq!(compacted.unwrap_err().to_string(), "nothing to compact");
    assert!(api.requests().await.is_empty());
}

#[tokio::test]
async fn answers_the_prompt_when_automatic_compaction_fails() {
    let api = MockApi::start(vec![
        full_context(text_reply("hello")),
        stopped_reply("partial summary", "refusal"),
        text_reply("ok"),
    ])
    .await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
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
    let mut agent = test_support::agent(&api, reqwest::Client::new());
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

#[tokio::test]
async fn does_not_summarise_again_when_the_request_after_compaction_is_too_long() {
    let api = MockApi::start_with_message_limit(
        vec![
            full_context(tool_reply("call-1", "bash", &json!({ "command": "true" }))),
            text_reply("summary of the work"),
            text_reply("second summary"),
        ],
        10_000,
    )
    .await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    agent.queue.lock().unwrap().push(Queued {
        prompt: "x".repeat(15_000),
        images: Vec::new(),
    });
    let mut notices = Vec::new();
    let result = agent
        .prompt("fix the bug", &[], test_support::notices(&mut notices))
        .await;
    test_support::remove_session(&agent);
    let message = result.unwrap_err().to_string();
    assert!(message.contains("prompt is too long"), "{message}");
    let compactions = notices
        .iter()
        .filter(|notice| *notice == "compacting conversation")
        .count();
    assert_eq!(compactions, 1, "{notices:?}");
}

#[tokio::test]
async fn compacts_a_conversation_that_outgrew_the_context_window() {
    // The tool output alone is larger than the API accepts.
    let api = MockApi::start_with_message_limit(
        vec![
            full_context(tool_reply(
                "call-1",
                "bash",
                &json!({ "command": "head -c 15000 /dev/zero | tr '\\0' x" }),
            )),
            text_reply("summary of the work"),
            text_reply("done"),
        ],
        10_000,
    )
    .await;
    let mut notices = Vec::new();
    let (agent, result) = test_support::run_prompt(
        &api,
        "read the big file",
        test_support::notices(&mut notices),
    )
    .await;
    result.unwrap();
    assert!(
        notices.iter().any(|notice| notice.contains("too long")),
        "{notices:?}"
    );
    let requests = api.requests().await;
    let last = requests.last().unwrap()["messages"].to_string();
    assert!(last.contains("summary of the work"), "{last}");
    // The session keeps the full tool output.
    let output = "x".repeat(15_000);
    assert!(
        serde_json::Value::from(agent.messages())
            .to_string()
            .contains(&output)
    );
}

#[tokio::test]
async fn compacts_when_tool_output_makes_the_next_request_too_long() {
    let api = MockApi::start_with_message_limit(
        vec![
            tool_reply(
                "call-1",
                "bash",
                &json!({ "command": "head -c 15000 /dev/zero | tr '\\0' x" }),
            ),
            text_reply("summary of the work"),
            text_reply("done"),
        ],
        10_000,
    )
    .await;
    let (_, result) = test_support::run_prompt(&api, "read the big file", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    let last = requests.last().unwrap()["messages"].to_string();
    assert!(last.contains("summary of the work"), "{last}");
}

#[tokio::test]
async fn compacts_only_once_when_a_paused_reply_makes_the_next_request_too_long() {
    let api = MockApi::start_with_message_limit(
        vec![
            tool_reply(
                "call-1",
                "bash",
                &json!({ "command": "head -c 15000 /dev/zero | tr '\\0' x" }),
            ),
            text_reply("summary of the work"),
            stopped_reply(&"y".repeat(15_000), "pause_turn"),
            text_reply("second summary"),
        ],
        10_000,
    )
    .await;
    let mut notices = Vec::new();
    let (_, result) = test_support::run_prompt(
        &api,
        "read the big file",
        test_support::notices(&mut notices),
    )
    .await;
    let message = result.unwrap_err().to_string();
    assert!(message.contains("prompt is too long"), "{message}");
    let compactions = notices
        .iter()
        .filter(|notice| *notice == "compacting conversation")
        .count();
    assert_eq!(compactions, 1, "{notices:?}");
}
