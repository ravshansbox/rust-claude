use super::*;

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
