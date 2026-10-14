use super::*;

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
async fn expands_environment_variables_in_http_headers() {
    let (url, requests) = http_server(http_mcp).await;
    let mut config = http_config(&url);
    config
        .headers
        .insert("X-Home".into(), "home=${HOME}, cost=$5".into());
    let mut mcp = started(config).await;
    let initialize = &requests_for(&requests, "initialize")[0];
    assert_eq!(
        initialize.headers["x-home"],
        format!("home={}, cost=$5", std::env::var("HOME").unwrap())
    );
    let mut config = http_config(&url);
    config.headers.insert(
        "Authorization".into(),
        "Bearer ${RUST_CODING_AGENT_UNSET_TOKEN}".into(),
    );
    assert_eq!(
        mcp.add(start("unset".into(), Scope::Global, config).await)
            .status,
        "MCP server unset failed: header Authorization uses RUST_CODING_AGENT_UNSET_TOKEN, which is not set"
    );
}

#[tokio::test]
async fn sends_headers_from_the_headers_helper() {
    let (url, requests) = http_server(http_mcp).await;
    let mut config = http_config(&url);
    config.headers.insert("X-Static".into(), "kept".into());
    config
        .headers
        .insert("Authorization".into(), "Bearer static".into());
    config.headers_helper = Some(
            r#"printf '{"Authorization": "Bearer %s %s"}' "$CLAUDE_CODE_MCP_SERVER_NAME" "$CLAUDE_CODE_MCP_SERVER_URL""#
                .into(),
        );
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, config).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
    for method in ["initialize", "tools/call"] {
        let request = &requests_for(&requests, method)[0];
        assert_eq!(
            request.headers["authorization"],
            format!("Bearer web {url}")
        );
        assert_eq!(request.headers["x-static"], "kept");
    }
}

#[tokio::test]
async fn reports_a_failing_headers_helper() {
    let (url, _) = http_server(http_mcp).await;
    let mut mcp = Mcp::default();
    for (helper, reason) in [
        (
            "echo oops >&2; exit 3",
            "headersHelper exited with status 3: oops",
        ),
        ("exit 4", "headersHelper exited with status 4"),
        ("kill -9 $$", "headersHelper exited with a signal"),
        (
            "echo '[1]'",
            "headersHelper must print a JSON object of strings",
        ),
        (
            r#"echo '{"A": 1}'"#,
            "headersHelper must print a JSON object of strings",
        ),
        (
            r#"echo '{"A B": "c"}'"#,
            "headersHelper gave invalid header name \"A B\"",
        ),
    ] {
        let mut config = http_config(&url);
        config.headers_helper = Some(helper.into());
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, config).await)
                .status,
            format!("MCP server web failed: {reason}"),
            "{helper}"
        );
    }
}

#[tokio::test]
async fn stops_what_a_headers_helper_left_running() {
    let (url, requests) = http_server(http_mcp).await;
    let pid_file = TemporaryFile::new("mcp-helper-child", "");
    let mut config = http_config(&url);
    config.headers_helper = Some(format!(
        r#"sleep 30 & echo $! > '{}'; echo '{{"X-Helper": "ran"}}'"#,
        pid_file.path().display()
    ));
    let started = tokio::time::timeout(
        Duration::from_secs(5),
        start("web".into(), Scope::Global, config),
    )
    .await
    .expect("waited for what the headersHelper left running");
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(started).status,
        "loaded global MCP server: web (1 tool)"
    );
    assert_eq!(
        requests_for(&requests, "initialize")[0].headers["x-helper"],
        "ran"
    );
    assert_eq!(wait_until_gone(pid_file.content().trim()).await, "");
}

#[tokio::test]
async fn does_not_sign_in_with_oauth_when_a_headers_helper_is_set() {
    let (url, _) = http_server(|_| HttpReply::status(401, "")).await;
    let mut config = http_config(&url);
    config.headers_helper = Some("echo '{}'".into());
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, config).await)
            .status,
        "MCP server web failed: server answered initialize with status 401"
    );
}

#[tokio::test]
async fn runs_the_headers_helper_again_after_401_and_for_a_new_session() {
    let counter = TemporaryFile::new("mcp-helper-count", "0");
    let sessions = Arc::new(AtomicU64::new(0));
    let issued = sessions.clone();
    let (url, requests) = http_server(move |request| {
        let session = request.headers.get("mcp-session-id").map(String::as_str);
        let token = request.headers.get("authorization").map(String::as_str);
        match request.body["method"].as_str() {
            Some("initialize") => {
                let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
                http_mcp(request).with_header("mcp-session-id", &format!("s{number}"))
            }
            Some("tools/call") if token == Some("Bearer t1") => HttpReply::status(401, ""),
            Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
            _ => http_mcp(request),
        }
    })
    .await;
    let mut config = http_config(&url);
    config.headers_helper = Some(format!(
        r#"n=$(($(cat '{0}' 2>/dev/null || echo 0) + 1)); echo $n > '{0}'; printf '{{"Authorization": "Bearer t%s"}}' $n"#,
        counter.path().display()
    ));
    let mcp = started(config).await;
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
    let tokens = |method: &str| -> Vec<String> {
        requests_for(&requests, method)
            .iter()
            .map(|request| request.headers["authorization"].clone())
            .collect()
    };
    assert_eq!(tokens("initialize"), ["Bearer t1", "Bearer t3"]);
    assert_eq!(
        tokens("tools/call"),
        ["Bearer t1", "Bearer t2", "Bearer t3"]
    );
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
