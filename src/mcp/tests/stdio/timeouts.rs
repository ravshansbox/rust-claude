use super::*;

#[tokio::test]
async fn shows_server_errors_when_a_call_times_out() {
    let mcp = started_with_short_timeout(server_answering_calls_with("echo stuck >&2")).await;
    assert_eq!(
        call_test_echo(&mcp).await,
        Some(Err(
            "tools/call timed out after 1 seconds\nstderr:\nstuck".into()
        ))
    );
}

#[tokio::test]
async fn keeps_whole_characters_in_the_end_of_server_errors() {
    let mcp = started_with_short_timeout(server_answering_calls_with(&format!(
        "echo '{}' >&2",
        "é".repeat(2_000)
    )))
    .await;
    assert_eq!(
        call_test_echo(&mcp).await,
        Some(Err(format!(
            "tools/call timed out after 1 seconds\nstderr:\n{}",
            "é".repeat(1_999)
        )))
    );
}

#[tokio::test]
async fn tells_the_server_when_a_call_times_out() {
    let log = TemporaryFile::new("mcp-cancel", "");
    let mcp = started_with_short_timeout(server_with(
        &format!(
            r#"    *'"notifications/cancelled"'*) printf '%s\n' "$line" >> '{}' ;;"#,
            log.path().display()
        ),
        ":",
    ))
    .await;
    assert!(call_test_echo(&mcp).await.unwrap().is_err());
    let logged = eventually(|| log.content(), |logged| !logged.is_empty()).await;
    let message: Value = serde_json::from_str(logged.trim()).unwrap();
    assert_eq!(message["method"], "notifications/cancelled");
    assert_eq!(message["params"]["requestId"], 3);
    assert!(message["params"]["reason"].is_string());
}

#[tokio::test]
async fn times_out_when_the_server_stops_reading() {
    let mcp = started_with_short_timeout(server_answering_calls_with("sleep 30")).await;
    let timed_out = Some(Err("tools/call timed out after 1 seconds".into()));
    assert_eq!(call_test_echo(&mcp).await, timed_out);
    let large = json!({ "text": "a".repeat(1_000_000) });
    let call = tokio::time::timeout(Duration::from_secs(10), mcp.call("mcp__test__echo", &large));
    assert_eq!(call.await.ok(), Some(timed_out));
}

#[tokio::test]
async fn keeps_working_after_a_large_call_is_cancelled_mid_write() {
    let mcp = started(server_answering_calls_with(
        r#"case "$line" in *'"slow"'*) sleep 1 ;; esac
       id=${line#'{"id":'}; id=${id%%,*}
       printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
    ))
    .await;
    let cancel = Duration::from_millis(200);
    let slow = json!({ "slow": true });
    assert!(
        tokio::time::timeout(cancel, mcp.call("mcp__test__echo", &slow))
            .await
            .is_err()
    );
    let large = json!({ "text": "a".repeat(200_000) });
    assert!(
        tokio::time::timeout(cancel, mcp.call("mcp__test__echo", &large))
            .await
            .is_err()
    );
    assert_eq!(call_test_echo(&mcp).await, Some(Ok("pong".into())));
}
