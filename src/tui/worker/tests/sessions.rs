use super::*;

#[tokio::test]
async fn starts_lists_and_resumes_sessions() {
    let api = MockApi::start(vec![
        test_support::text_reply("one"),
        test_support::text_reply("two"),
        test_support::text_reply("three"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let first_id = agent.session.id.clone();
    let second_prompt = format!("after {first_id}");
    let mut worker = Worker::spawn(agent);
    worker.prompt("first", "off");
    let first = worker.wait_for_done().await;
    worker.requests.send(Request::NewSession).unwrap();
    worker.prompt(&second_prompt, "off");
    let second = worker.wait_for_done().await;
    worker.requests.send(Request::ListSessions).unwrap();
    let listed = worker
        .wait_for(|event| {
            matches!(event, UiEvent::Sessions(Ok(sessions))
                if sessions.iter().any(|session| session.id == first_id))
        })
        .await;
    worker
        .requests
        .send(Request::Resume(first_id.clone()))
        .unwrap();
    let resumed = worker
        .wait_for(|event| {
            matches!(event, UiEvent::Resumed(Ok(messages))
                if serde_json::to_string(messages).unwrap().contains("first"))
        })
        .await;
    worker.requests.send(Request::ListSessions).unwrap();
    let mut second_id = None;
    worker
        .wait_for(|event| {
            let UiEvent::Sessions(Ok(sessions)) = event else {
                return false;
            };
            second_id = sessions
                .iter()
                .find(|session| session.preview.contains(&second_prompt))
                .map(|session| session.id.clone());
            true
        })
        .await;
    worker.prompt("third", "off");
    let third = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&first_id);
    if let Some(id) = &second_id {
        test_support::remove_session_files(id);
    }
    assert!(first && second && listed && resumed && third);
    assert!(second_id.is_some());
    let sent: Vec<String> = api
        .requests()
        .await
        .iter()
        .map(|request| request["messages"].to_string())
        .collect();
    assert!(!sent[1].contains("first"), "{}", sent[1]);
    assert!(sent[2].contains("first") && sent[2].contains("third"));
    assert!(!sent[2].contains(&second_prompt), "{}", sent[2]);
}

#[tokio::test]
async fn compacts_the_conversation_and_continues_from_the_summary() {
    let api = MockApi::start(vec![
        test_support::text_reply("one"),
        test_support::text_reply("the summary"),
        test_support::text_reply("two"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker.prompt("first", "medium");
    let first = worker.wait_for_done().await;
    worker.requests.send(Request::Compact("high")).unwrap();
    let compacted = worker.wait_for_done().await;
    worker.prompt("second", "medium");
    let second = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(first && compacted && second);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["output_config"]["effort"], "high");
    let sent = requests[2]["messages"].to_string();
    assert!(
        sent.contains("the summary") && sent.contains("second"),
        "{sent}"
    );
    assert!(!sent.contains("first"), "{sent}");
}

#[tokio::test]
async fn returns_the_session_to_restart_with_once_it_has_messages() {
    let api = MockApi::start(vec![test_support::text_reply("one")]).await;
    let agent = test_support::agent(&api);
    let id = agent.session.id.clone();
    let worker = Worker::spawn(test_support::agent(&api));
    assert_eq!(worker.quit().await.unwrap(), None);
    let mut worker = Worker::spawn(agent);
    worker.prompt("first", "off");
    assert!(worker.wait_for_done().await);
    let returned = worker.quit().await.unwrap();
    test_support::remove_session_files(&id);
    assert_eq!(returned, Some(id));
}

#[tokio::test]
async fn sends_a_loop_prompt_in_a_new_session() {
    let api = MockApi::start(vec![
        test_support::text_reply("one"),
        test_support::text_reply("two"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let first = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker.prompt("go", "off");
    assert!(worker.wait_for_done().await);
    worker
        .requests
        .send(Request::LoopPrompt("go".into(), Vec::new(), "off"))
        .unwrap();
    assert!(worker.wait_for_done().await);
    let second = worker.quit().await.unwrap().unwrap();
    test_support::remove_session_files(&first);
    test_support::remove_session_files(&second);
    assert_ne!(first, second);
    let requests = api.requests().await;
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 1);
}
