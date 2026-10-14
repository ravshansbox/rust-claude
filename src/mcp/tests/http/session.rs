use super::*;

#[tokio::test]
async fn starts_a_new_http_session_when_the_old_one_expires() {
    let sessions = Arc::new(AtomicU64::new(0));
    let issued = sessions.clone();
    let (url, requests) = http_server(move |request| {
        let session = request.headers.get("mcp-session-id").map(String::as_str);
        match request.body["method"].as_str() {
            Some("initialize") => {
                let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
                http_mcp(request).with_header("mcp-session-id", &format!("s{number}"))
            }
            Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
            _ => http_mcp(request),
        }
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
    let sessions_used = |method: &str| -> Vec<String> {
        requests_for(&requests, method)
            .iter()
            .map(|request| {
                request
                    .headers
                    .get("mcp-session-id")
                    .cloned()
                    .unwrap_or_default()
            })
            .collect()
    };
    assert_eq!(sessions_used("initialize"), ["", ""]);
    assert_eq!(sessions_used("notifications/initialized"), ["s1", "s2"]);
    assert_eq!(sessions_used("tools/call"), ["s1", "s2"]);
}

#[tokio::test]
async fn waits_for_a_new_http_session_before_sending_other_calls() {
    let sessions = Arc::new(AtomicU64::new(0));
    let issued = sessions.clone();
    let (url, _requests) = http_server(move |request| {
        let session = request.headers.get("mcp-session-id").map(String::as_str);
        match request.body["method"].as_str() {
            Some("initialize") => {
                let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
                http_mcp(request)
                    .with_header("mcp-session-id", &format!("s{number}"))
                    .after(Duration::from_millis(if number == 1 { 0 } else { 300 }))
            }
            _ if session.is_none() => HttpReply::status(400, ""),
            Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
            _ => http_mcp(request),
        }
    })
    .await;
    let mcp = started(http_config(&url)).await;
    let later = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        call_web_echo(&mcp).await
    };
    let (first, second) = tokio::join!(call_web_echo(&mcp), later);
    assert_eq!(first, Some(Ok("pong".into())));
    assert_eq!(second, Some(Ok("pong".into())));
}

#[tokio::test]
async fn tries_a_new_http_session_again_after_a_failed_renewal() {
    let sessions = Arc::new(AtomicU64::new(0));
    let issued = sessions.clone();
    let (url, _requests) = http_server(move |request| {
        let session = request.headers.get("mcp-session-id").map(String::as_str);
        match request.body["method"].as_str() {
            Some("initialize") => match issued.fetch_add(1, Ordering::SeqCst) + 1 {
                2 => HttpReply::status(503, ""),
                number => http_mcp(request).with_header("mcp-session-id", &format!("s{number}")),
            },
            _ if session.is_none() => HttpReply::status(400, ""),
            Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
            _ => http_mcp(request),
        }
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert!(matches!(call_web_echo(&mcp).await, Some(Err(_))));
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
}

#[tokio::test]
async fn ends_the_http_session_when_dropped() {
    let (url, requests) = http_server(http_mcp).await;
    let mcp = started(http_config(&url)).await;
    drop(mcp);
    let deleted = || -> Vec<(String, Option<String>)> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == "DELETE")
            .map(|request| {
                (
                    request.headers["mcp-session-id"].clone(),
                    request.headers.get("mcp-protocol-version").cloned(),
                )
            })
            .collect()
    };
    let deleted = eventually(deleted, |deleted| !deleted.is_empty()).await;
    assert_eq!(deleted, [("s1".into(), Some("2025-03-26".into()))]);
}
