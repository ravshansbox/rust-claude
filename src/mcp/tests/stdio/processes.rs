use super::*;

#[tokio::test]
async fn reaps_a_server_that_exits_mid_session() {
    let pid_file = TemporaryFile::new("mcp-reap", "");
    let config = server_answering_calls_with(&format!(
        "echo $$ > '{}'; exit 0",
        pid_file.path().display()
    ));
    let mcp = started(config).await;
    assert!(call_test_echo(&mcp).await.unwrap().is_err());
    let pid = pid_file.content();
    assert_eq!(
        wait_until_gone(pid.trim()).await,
        "",
        "server was not reaped"
    );
    drop(mcp);
}

#[tokio::test]
async fn stops_what_a_server_left_running_when_it_exits() {
    let pid_file = TemporaryFile::new("mcp-leftover", "");
    let config = server_answering_calls_with(&format!(
        "sleep 30 >/dev/null 2>&1 & echo $! > '{}'; exit 0",
        pid_file.path().display()
    ));
    let mcp = started(config).await;
    assert!(call_test_echo(&mcp).await.unwrap().is_err());
    let pid = pid_file.content();
    assert_eq!(
        wait_until_gone(pid.trim()).await,
        "",
        "leftover process kept running"
    );
    drop(mcp);
}

#[tokio::test]
async fn stops_the_server_when_dropped() {
    let pid_file = TemporaryFile::new("mcp-drop", "");
    let mcp = started_with_short_timeout(server_answering_calls_with(&format!(
        "echo $$ > '{}'; sleep 30",
        pid_file.path().display()
    )))
    .await;
    let _ = call_test_echo(&mcp).await;
    let pid = pid_file.content();
    drop(mcp);
    assert_eq!(wait_until_gone(pid.trim()).await, "");
}
