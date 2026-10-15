use super::*;

mod headers;
mod replies;
mod session;

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

async fn started(config: ServerConfig) -> Mcp {
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, config).await);
    mcp
}

async fn started_with_short_timeout(mut config: ServerConfig) -> Mcp {
    config.timeout = Some(1);
    started(config).await
}
