use super::*;

#[tokio::test]
async fn gives_each_sequential_task_the_previous_final_reply() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("list files", "list the files"), task("pick one", "pick a file")],
            }),
        ),
        text_reply("a.rs and b.rs"),
        text_reply("b.rs"),
        text_reply("done"),
    ])
    .await;
    let (_, result) = test_support::run_prompt(&api, "chain", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[2]["messages"][0]["content"],
        json!([{
            "type": "text",
            "text": "pick a file\n\n<previous-task-result>\na.rs and b.rs\n</previous-task-result>",
            "cache_control": { "type": "ephemeral" },
        }])
    );
    assert_eq!(
        tool_result(&requests[3])["content"],
        json!([
            { "type": "text", "text": "a.rs and b.rs" },
            { "type": "text", "text": "b.rs" },
        ])
    );
}

#[tokio::test]
async fn stops_a_sequence_at_the_task_that_failed() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        Reply::BadRequest("prompt is too long".into()),
        text_reply("done"),
    ])
    .await;
    let (_, result) = test_support::run_prompt(&api, "chain", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let content = tool_result(&requests[2])["content"].clone();
    let first = content[0]["text"].as_str().unwrap();
    assert!(first.starts_with("This task failed: "), "{first}");
    assert!(first.contains("prompt is too long"), "{first}");
    assert_eq!(
        content[1]["text"],
        "This task did not run, because an earlier task failed."
    );
}

#[tokio::test]
async fn runs_parallel_tasks_at_the_same_time() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "parallel",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        Reply::Stall,
        Reply::Stall,
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut prompt = Box::pin(agent.prompt("both", &[], |_| {}));
    let both_sent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut prompt => return false,
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if api.requests().await.len() == 3 {
                        return true;
                    }
                }
            }
        }
    })
    .await;
    drop(prompt);
    test_support::remove_session(&agent);
    assert_eq!(both_sent, Ok(true));
}
