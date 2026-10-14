use super::*;

mod headers;

#[tokio::test]
async fn sums_up_http_servers_without_a_sign_in_as_anonymous() {
    let (url, _) = http_server(http_mcp).await;
    let mut mcp = Mcp::default();
    let added = mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert_eq!(added.summary, "web (anonymous, 1 tool)");
    let (url, _) = http_server(http_mcp).await;
    let mut config = http_config(&url);
    config
        .headers
        .insert("Authorization".into(), "Bearer secret".into());
    let added = mcp.add(start("keyed".into(), Scope::Global, config).await);
    assert_eq!(added.summary, "keyed (1 tool)");
}

#[tokio::test]
async fn calls_http_server_tools() {
    let (url, requests) = http_server(http_mcp).await;
    let mut config = http_config(&url);
    config.kind = Some("http".into());
    config
        .headers
        .insert("Authorization".into(), "Bearer secret".into());
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, config).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
    let initialize = &requests_for(&requests, "initialize")[0];
    assert_eq!(initialize.method, "POST");
    assert_eq!(initialize.headers["authorization"], "Bearer secret");
    assert_eq!(
        initialize.headers["accept"],
        "application/json, text/event-stream"
    );
    assert!(!initialize.headers.contains_key("mcp-session-id"));
    for method in ["notifications/initialized", "tools/list", "tools/call"] {
        let request = &requests_for(&requests, method)[0];
        assert_eq!(request.headers["mcp-session-id"], "s1", "{method}");
        assert_eq!(
            request.headers["mcp-protocol-version"], "2025-03-26",
            "{method}"
        );
        assert_eq!(request.headers["authorization"], "Bearer secret");
    }
}

#[tokio::test]
async fn reads_http_replies_sent_as_events() {
    let (url, requests) = http_server(|request| {
        let id = &request.body["id"];
        match request.body["method"].as_str() {
            Some("tools/call") => HttpReply::events(&[
                json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} }),
                json!({ "jsonrpc": "2.0", "id": "p1", "method": "ping" }),
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": "streamed" }] },
                }),
            ]),
            _ => http_mcp(request),
        }
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("streamed".into())));
    let replies = || -> Vec<Value> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.body["id"] == "p1")
            .map(|request| request.body.clone())
            .collect()
    };
    let replies = eventually(replies, |replies| !replies.is_empty()).await;
    assert_eq!(
        replies,
        [json!({ "jsonrpc": "2.0", "id": "p1", "result": {} })]
    );
}

#[tokio::test]
async fn reads_http_replies_sent_as_a_json_batch() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply::json(&json!([
            { "jsonrpc": "2.0", "method": "notifications/progress", "params": {} },
            {
                "jsonrpc": "2.0",
                "id": request.body["id"],
                "result": { "content": [{ "type": "text", "text": "batched" }] },
            },
        ])),
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("batched".into())));
}

#[tokio::test]
async fn rejects_http_replies_with_an_unsupported_content_type() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => {
            HttpReply::status(200, "pong").with_header("content-type", "Text/Plain; charset=utf-8")
        }
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(
        call_web_echo(&mcp).await,
        Some(Err(
            "server answered tools/call with unsupported content type \"text/plain\"".into()
        ))
    );
}

#[tokio::test]
async fn reads_events_split_over_lines_with_crlf_endings_and_an_empty_type() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: format!(
                ": comment\r\nevent: other\r\ndata: {{}}\r\n\r\nevent:\r\ndata: {{\"jsonrpc\": \"2.0\", \"id\": {},\r\ndata: \"result\": {{\"content\": [{{\"type\": \"text\", \"text\": \"split\"}}]}}}}\r\n\r\n",
                request.body["id"]
            ),
            delay: Duration::ZERO,
        },
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("split".into())));
}

#[tokio::test]
async fn reads_events_with_cr_line_endings() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: format!(
                "event: other\rdata: {{}}\r\rdata: {{\"jsonrpc\": \"2.0\", \"id\": {}, \"result\": {{\"content\": [{{\"type\": \"text\", \"text\": \"cr\"}}]}}}}\r\r",
                request.body["id"]
            ),
            delay: Duration::ZERO,
        },
        _ => http_mcp(request),
    })
    .await;
    let mcp = started(http_config(&url)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("cr".into())));
}

#[tokio::test]
async fn fails_calls_when_an_http_reply_is_too_long() {
    for content_type in ["application/json", "text/event-stream"] {
        let (url, _) = http_server(move |request| match request.body["method"].as_str() {
            Some("tools/call") => HttpReply {
                status: 200,
                headers: vec![("content-type".into(), content_type.into())],
                body: format!("data: {}", "a".repeat(MAX_LINE + 1)),
                delay: Duration::ZERO,
            },
            _ => http_mcp(request),
        })
        .await;
        let mut config = http_config(&url);
        config.timeout = Some(60);
        let mcp = started(config).await;
        assert_eq!(
            call_web_echo(&mcp).await,
            Some(Err("server sent a reply longer than 32 MiB".into())),
            "{content_type}"
        );
    }
}

#[tokio::test]
async fn reports_http_errors() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply::status(202, ""),
        Some("tools/list") => HttpReply::status(500, "database is down"),
        _ => http_mcp(request),
    })
    .await;
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "MCP server web failed: server answered tools/list with status 500: database is down"
    );
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply::status(202, ""),
        _ => http_mcp(request),
    })
    .await;
    mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert_eq!(
        call_web_echo(&mcp).await,
        Some(Err("server accepted tools/call without a reply".into()))
    );
}

#[tokio::test]
async fn times_out_when_an_http_server_does_not_answer() {
    let (url, _) = http_server(|request| match request.body["method"].as_str() {
        Some("tools/call") => HttpReply::stall(),
        _ => http_mcp(request),
    })
    .await;
    let mcp = started_with_short_timeout(http_config(&url)).await;
    assert_eq!(
        call_web_echo(&mcp).await,
        Some(Err("tools/call timed out after 1 seconds".into()))
    );
}

#[tokio::test]
async fn stops_waiting_for_an_http_notification_the_server_does_not_answer() {
    let calls = AtomicU64::new(0);
    let (url, _) = http_server(move |request| match request.body["method"].as_str() {
        Some("tools/call") if calls.fetch_add(1, Ordering::SeqCst) == 0 => HttpReply::stall(),
        Some("notifications/cancelled") => HttpReply::stall(),
        _ => http_mcp(request),
    })
    .await;
    let mcp = started_with_short_timeout(http_config(&url)).await;
    assert_eq!(
        call_web_echo(&mcp).await,
        Some(Err("tools/call timed out after 1 seconds".into()))
    );
    // The cancellation of that call stalls until its own timeout.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
}

#[tokio::test]
async fn retries_connecting_after_a_transient_http_error() {
    let attempts = Arc::new(AtomicU64::new(0));
    let counted = attempts.clone();
    let (url, requests) = http_server(move |request| {
        if request.body["method"] == "initialize" && counted.fetch_add(1, Ordering::SeqCst) < 2 {
            return HttpReply::status(503, "starting up");
        }
        http_mcp(request)
    })
    .await;
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    assert_eq!(requests_for(&requests, "initialize").len(), 3);
    let (url, requests) = http_server(|request| match request.body["method"].as_str() {
        Some("initialize") => HttpReply::status(400, "bad request"),
        _ => http_mcp(request),
    })
    .await;
    assert_eq!(
        mcp.add(start("locked".into(), Scope::Global, http_config(&url)).await)
            .status,
        "MCP server locked failed: server answered initialize with status 400: bad request"
    );
    assert_eq!(requests_for(&requests, "initialize").len(), 1);
}

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
async fn rejects_urls_that_are_not_http() {
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(
            start(
                "ftp".into(),
                Scope::Global,
                http_config("ftp://example.com")
            )
            .await
        )
        .status,
        "MCP server ftp failed: url must be an http or https URL"
    );
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

async fn started(config: ServerConfig) -> Mcp {
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, config).await);
    mcp
}

async fn started_with_short_timeout(mut config: ServerConfig) -> Mcp {
    config.timeout = Some(1);
    started(config).await
}
