use super::*;

#[tokio::test]
async fn renews_oauth_tokens_that_an_http_server_turns_down() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = http_server(signed_in_mcp).await;
    save_tokens(&url, "stale", now_millis() + 3_600_000);
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
    let saved = oauth::load(&url).unwrap();
    assert_eq!(saved.access_token, "fresh");
    assert_eq!(saved.refresh_token.as_deref(), Some("r2"));
    let tokens_sent: Vec<String> = requests_for(&requests, "initialize")
        .iter()
        .map(|request| request.headers["authorization"].clone())
        .collect();
    assert_eq!(tokens_sent, ["Bearer stale", "Bearer fresh"]);
}

#[tokio::test]
async fn keeps_renewed_oauth_tokens_that_could_not_be_saved() {
    let _file = AUTH_FILE.write().await;
    let (url, requests) = http_server(signed_in_mcp).await;
    let expires_at = now_millis() + 2_000;
    save_tokens(&url, "fresh", expires_at);
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    let path = config::dir().unwrap().join("mcp-auth.json");
    let aside = path.with_extension("aside");
    std::fs::rename(&path, &aside).unwrap();
    // A folder in the way of mcp-auth.json makes saving fail.
    std::fs::create_dir(&path).unwrap();
    let wait = expires_at.saturating_sub(now_millis()) + 10;
    tokio::time::sleep(Duration::from_millis(wait.try_into().unwrap())).await;
    let called = call_web_echo(&mcp).await;
    let notices = mcp.take_renewal_notices();
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&aside, &path).unwrap();
    assert_eq!(called, Some(Ok("pong".into())));
    assert_eq!(requests_for_path(&requests, "/token").len(), 1);
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].starts_with("renewed the sign-in to MCP server web, but cannot save"),
        "{notices:?}"
    );
    assert_eq!(mcp.take_renewal_notices(), Vec::<String>::new());
}

#[tokio::test]
async fn sends_the_client_secret_from_the_config_when_renewing() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = http_server(signed_in_mcp).await;
    save_tokens(&url, "stale", now_millis() - 1);
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("client".into()),
        client_secret: Some("own-secret".into()),
    });
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, config).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    let token_form = requests_for_path(&requests, "/token")[0].form();
    assert_eq!(token_form["client_secret"], "own-secret");
    let file = std::fs::read_to_string(config::dir().unwrap().join("mcp-auth.json")).unwrap();
    assert!(!file.contains("own-secret"), "{file}");
}

#[tokio::test]
async fn keeps_the_client_secret_from_the_config_when_taking_over_tokens() {
    let _file = AUTH_FILE.read().await;
    let (url, _) = http_server(signed_in_mcp).await;
    save_tokens(&url, "mine", now_millis() - 1);
    let config = OAuthConfig {
        client_id: Some("client".into()),
        client_secret: Some("own-secret".into()),
    };
    let mine = oauth::load_for(&url, Some(&config)).unwrap();
    save_tokens(&url, "theirs", now_millis() + 3_600_000);
    let taken_over = oauth::refresh(&reqwest::Client::new(), &url, &mine, None)
        .await
        .unwrap();
    assert_eq!(taken_over.saved.access_token, "theirs");
    assert_eq!(
        taken_over.saved.client_secret.as_deref(),
        Some("own-secret")
    );
}

#[tokio::test]
async fn running_copies_renew_an_mcp_sign_in_one_at_a_time() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = http_server(signed_in_mcp).await;
    save_tokens(&url, "stale", now_millis() - 1);
    let stale = oauth::load(&url).unwrap();
    let client = reqwest::Client::new();
    let (one, other) = tokio::join!(
        oauth::refresh(&client, &url, &stale, None),
        oauth::refresh(&client, &url, &stale, None),
    );
    assert_eq!(requests_for_path(&requests, "/token").len(), 1);
    assert_eq!(one.unwrap().saved.access_token, "fresh");
    assert_eq!(other.unwrap().saved.access_token, "fresh");
}

#[tokio::test]
async fn uses_an_mcp_sign_in_another_copy_renewed_while_ours_was_turned_down() {
    let _file = AUTH_FILE.read().await;
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        if request.path == "/token" {
            // A copy that does not take turns renews first, spending r1.
            save_tokens(&shared.lock().unwrap(), "theirs", now_millis() + 3_600_000);
            return HttpReply::status(400, r#"{"error":"invalid_grant"}"#);
        }
        signed_in_mcp(request)
    })
    .await;
    base.lock().unwrap().clone_from(&url);
    save_tokens(&url, "stale", now_millis() - 1);
    let stale = oauth::load(&url).unwrap();
    let renewed = oauth::refresh(&reqwest::Client::new(), &url, &stale, None).await;
    assert_eq!(renewed.unwrap().saved.access_token, "theirs");
}

#[tokio::test]
async fn renews_expired_oauth_tokens_before_sending_them() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = http_server(signed_in_mcp).await;
    save_tokens(&url, "stale", now_millis() - 1);
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "loaded global MCP server: web (1 tool)"
    );
    let tokens_sent: Vec<String> = requests_for(&requests, "initialize")
        .iter()
        .map(|request| request.headers["authorization"].clone())
        .collect();
    assert_eq!(tokens_sent, ["Bearer fresh"]);
}

#[tokio::test]
async fn uses_short_lived_oauth_tokens_until_they_expire() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = http_server(|request| {
        if request.path == "/token" {
            return HttpReply::json(&json!({
                "access_token": "fresh",
                "token_type": "Bearer",
                "expires_in": 30,
            }));
        }
        signed_in_mcp(request)
    })
    .await;
    save_tokens(&url, "stale", now_millis() - 1);
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert_eq!(call_web_echo(&mcp).await, Some(Ok("pong".into())));
    assert_eq!(requests_for_path(&requests, "/token").len(), 1);
}
