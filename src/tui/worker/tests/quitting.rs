use super::*;

#[tokio::test]
async fn does_not_start_requests_sent_before_quitting() {
    let marker =
        std::env::temp_dir().join(format!("rust-coding-agent-quit-{}", std::process::id()));
    let agent = agent();
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    request_tx
        .send(Request::Shell(format!("touch {}", marker.display()), true))
        .unwrap();
    quit(request_tx, cancel_tx);
    agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx).await;
    let ran = marker.exists();
    let _ = std::fs::remove_file(&marker);
    test_support::remove_session_files(&session_id);
    assert!(!ran);
}

#[tokio::test]
async fn stops_running_request_when_quitting() {
    let agent = agent();
    let session_id = agent.session.id.clone();
    let worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Shell("sleep 30".into(), true))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker.quit()).await;
    test_support::remove_session_files(&session_id);
    assert!(stopped.is_ok());
}

#[tokio::test]
async fn quits_while_a_sign_in_still_holds_a_request_sender() {
    let agent = agent();
    let session_id = agent.session.id.clone();
    let worker = Worker::spawn(agent);
    let _signing_in = worker.requests.clone();
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker.quit()).await;
    test_support::remove_session_files(&session_id);
    assert!(stopped.is_ok());
}
