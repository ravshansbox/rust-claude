use super::*;

#[tokio::test]
async fn counts_the_tokens_subagents_use_in_the_totals() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "parallel",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        text_reply("one done"),
        text_reply("two done"),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut inputs = Vec::new();
    let result = agent
        .prompt("go", &[], |event| {
            if let AgentEvent::Stats(stats) = event {
                inputs.push(stats.usage.input);
            }
        })
        .await;
    let context_tokens = agent.stats().context_tokens;
    let id = agent.session.id.clone();
    drop(agent);
    let mut resumed = test_support::agent(&api);
    let loaded = resumed.resume(&id).await;
    test_support::remove_session(&resumed);
    result.unwrap();
    loaded.unwrap();
    assert!(inputs.contains(&3), "{inputs:?}");
    assert_eq!(inputs.last(), Some(&4));
    let usage = resumed.stats().usage;
    assert_eq!((usage.input, usage.output), (4, 4));
    assert_eq!(resumed.stats().context_tokens, context_tokens);
    let requests = api.requests().await;
    assert!(!requests[3]["messages"].to_string().contains("usage"));
}

#[tokio::test]
async fn counts_the_tokens_of_a_subagent_that_failed() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({ "mode": "parallel", "tasks": [task("first", "one")] }),
        ),
        test_support::stopped_reply("partial", "refusal"),
        text_reply("done"),
    ])
    .await;
    let (agent, result) = test_support::run_prompt(&api, "go", |_| {}).await;
    result.unwrap();
    assert_eq!(agent.stats().usage.input, 3);
}

#[tokio::test]
async fn counts_finished_subagent_replies_when_cancelled() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "parallel",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        text_reply("one done"),
        Reply::Stall,
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let one_done = std::cell::Cell::new(false);
    let mut prompt = Box::pin(agent.prompt("go", &[], |event| {
        if matches!(event, AgentEvent::TaskDone { reply: Ok(_), .. }) {
            one_done.set(true);
        }
    }));
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut prompt => return false,
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if one_done.get() && api.requests().await.len() == 3 {
                        return true;
                    }
                }
            }
        }
    })
    .await;
    drop(prompt);
    let cancelled = agent.cancel(0);
    test_support::remove_session(&agent);
    cancelled.unwrap();
    assert_eq!(ready, Ok(true));
    assert_eq!(agent.stats().usage.input, 2);
}
