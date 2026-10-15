use super::*;

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
    let mut agent = test_support::agent(&api);
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
