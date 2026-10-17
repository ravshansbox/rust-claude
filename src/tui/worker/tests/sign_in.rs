use super::*;

/// An agent whose sign-in has expired and whose renewal never answers.
async fn agent_renewing_sign_in() -> (Agent, MockApi) {
    let api = MockApi::start(vec![Reply::Stall]).await;
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&api.base).unwrap())
        .build()
        .unwrap();
    let agent = Agent::new(http, test_support::credentials(0), "model".into()).unwrap();
    (agent, api)
}

#[tokio::test]
async fn quits_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let worker = Worker::spawn(agent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker.quit()).await;
    assert!(stopped.is_ok());
}

#[tokio::test]
async fn says_when_it_renewed_the_sign_in_at_startup() {
    let _lock = AUTH_FILE.lock().await;
    answer_token_requests(
        "200 OK",
        json!({ "access_token": "new", "refresh_token": "next", "expires_in": 3600 }),
    )
    .await;
    let path = no_credentials_file();
    let mut agent = Agent::new(reqwest::Client::new(), expired("refresh"), "model".into()).unwrap();
    agent.api_base = "http://127.0.0.1:1".into();
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    let shown = worker
        .wait_for(|event| {
            matches!(event, UiEvent::Agent(AgentEvent::Notice(notice)) if notice == "renewed sign-in token")
        })
        .await;
    worker.quit().abort();
    test_support::remove_session_files(&session_id);
    let _ = std::fs::remove_file(&path);
    assert!(shown);
}

#[tokio::test]
async fn cancels_a_prompt_sent_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    worker.prompt("hello", "off");
    worker.cancel.send(()).unwrap();
    let cancelled = worker
        .wait_for(|event| matches!(event, UiEvent::Cancelled(Ok(()))))
        .await;
    worker.quit().abort();
    let session = config::dir()
        .map(|dir| dir.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap();
    let saved = std::fs::read_to_string(&session).unwrap_or_default();
    let _ = std::fs::remove_file(&session);
    assert!(cancelled);
    assert!(saved.contains("hello"), "session file: {saved:?}");
}

#[tokio::test]
async fn cancels_a_shell_command_sent_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let directory = TemporaryDir::new("worker-cancelled-shell");
    let marker = directory.path().join("ran");
    let mut worker = Worker::spawn(agent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    worker
        .requests
        .send(Request::Shell(
            format!("touch '{}'", marker.display()),
            true,
        ))
        .unwrap();
    worker.cancel.send(()).unwrap();
    let cancelled = worker
        .wait_for(|event| matches!(event, UiEvent::Cancelled(Ok(()))))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    worker.quit().abort();
    assert!(cancelled);
    assert!(!marker.exists());
}
