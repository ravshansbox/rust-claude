use super::*;

#[tokio::test]
async fn uses_an_effort_the_new_model_supports_for_background_turns() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "1",
            "bash",
            &json!({ "command": "sleep 0.5", "run_in_background": true }),
        ),
        test_support::text_reply("started"),
        test_support::text_reply("finished"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    agent.model = "claude-haiku-5-5".into();
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker.prompt("go", "off");
    worker
        .requests
        .send(Request::SetModel("claude-opus-5-5".into()))
        .unwrap();
    worker
        .wait_for(|event| matches!(event, UiEvent::Background(_)))
        .await;
    worker
        .wait_for(|event| matches!(event, UiEvent::Done(_)))
        .await;
    worker.quit().abort();
    test_support::remove_session_files(&session_id);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2]["model"], "claude-opus-5-5");
    assert_eq!(requests[2]["output_config"]["effort"], "medium");
}

#[tokio::test]
async fn uses_the_effort_chosen_since_the_last_prompt_for_background_turns() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "1",
            "bash",
            &json!({ "command": "sleep 0.5", "run_in_background": true }),
        ),
        test_support::text_reply("started"),
        test_support::text_reply("finished"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker.prompt("go", "medium");
    worker.requests.send(Request::SetEffort("high")).unwrap();
    worker
        .wait_for(|event| matches!(event, UiEvent::Background(_)))
        .await;
    worker
        .wait_for(|event| matches!(event, UiEvent::Done(_)))
        .await;
    worker.quit().abort();
    test_support::remove_session_files(&session_id);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["output_config"]["effort"], "medium");
    assert_eq!(requests[2]["output_config"]["effort"], "high");
}

#[tokio::test]
async fn starts_a_turn_when_a_background_command_finishes() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "call-1",
            "bash",
            &json!({ "command": "sleep 0.3; echo finished", "run_in_background": true }),
        ),
        test_support::text_reply("waiting"),
        test_support::text_reply("seen it"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker.prompt("watch it", "off");
    let first = worker.wait_for_done().await;
    let delivered = worker
        .wait_for(|event| matches!(event, UiEvent::Background(text) if text.contains("finished")))
        .await;
    let second = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(first && delivered && second);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let last = requests[2]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert!(last["content"].to_string().contains("finished"), "{last}");
}

#[tokio::test]
async fn runs_a_prompt_already_sent_before_a_finished_background_command() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "call-1",
            "bash",
            &json!({ "command": "echo finished", "run_in_background": true }),
        ),
        test_support::tool_reply("call-2", "bash", &json!({ "command": "sleep 0.3" })),
        test_support::text_reply("waiting"),
        test_support::text_reply("answered"),
        test_support::text_reply("seen it"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    for prompt in ["watch it", "meanwhile"] {
        worker.prompt(prompt, "off");
    }
    let delivered = worker
        .wait_for(|event| matches!(event, UiEvent::Background(_)))
        .await;
    let done = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(delivered && done);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 5);
    let last_sent = |request: &serde_json::Value| {
        let messages = request["messages"].as_array().unwrap();
        messages.last().unwrap()["content"].to_string()
    };
    assert!(last_sent(&requests[3]).contains("meanwhile"));
    assert!(last_sent(&requests[4]).contains("finished"));
}
