use super::*;

/// Tests share one mcp-auth.json: those that use it take turns with those
/// that make it unusable.
static AUTH_FILE: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Answers like `http_mcp`, but only with the access token `fresh`, and
/// renews the refresh token `r1` at `/token`.
fn signed_in_mcp(request: &HttpRequest) -> HttpReply {
    if request.path == "/token" {
        let form = request.form();
        if form.get("grant_type").map(String::as_str) == Some("refresh_token")
            && form.get("refresh_token").map(String::as_str) == Some("r1")
            && form.get("client_id").map(String::as_str) == Some("client")
        {
            return HttpReply::json(json!({
                "access_token": "fresh",
                "refresh_token": "r2",
                "token_type": "Bearer",
                "expires_in": 3600,
            }));
        }
        return HttpReply::status(400, r#"{"error":"invalid_grant"}"#);
    }
    if request.headers.get("authorization").map(String::as_str) != Some("Bearer fresh") {
        return HttpReply::status(401, "")
            .with_header("www-authenticate", r#"Bearer error="invalid_token""#);
    }
    http_mcp(request)
}

fn save_tokens(url: &str, access_token: &str, expires_at: u128) {
    let token_endpoint = url.replace("/mcp", "/token");
    oauth::save(
        url,
        &oauth::Saved {
            client_id: "client".into(),
            client_secret: None,
            token_endpoint,
            resource: Some(url.into()),
            access_token: access_token.into(),
            refresh_token: Some("r1".into()),
            expires_at: Some(expires_at),
            secret_from_config: false,
        },
    )
    .unwrap();
}

#[tokio::test]
async fn renews_oauth_tokens_that_an_http_server_turns_down() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = http_server(signed_in_mcp).await;
    save_tokens(&url, "stale", now_millis() + 3_600_000);
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "loaded global MCP server: web (1 tools)"
    );
    assert_eq!(
        mcp.call("mcp__web__echo", &json!({})).await,
        Some(Ok("pong".into()))
    );
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
        "loaded global MCP server: web (1 tools)"
    );
    let path = config::dir().unwrap().join("mcp-auth.json");
    let aside = path.with_extension("aside");
    std::fs::rename(&path, &aside).unwrap();
    // A folder in the way of mcp-auth.json makes saving fail.
    std::fs::create_dir(&path).unwrap();
    let wait = expires_at.saturating_sub(now_millis()) + 10;
    tokio::time::sleep(Duration::from_millis(wait.try_into().unwrap())).await;
    let called = mcp.call("mcp__web__echo", &json!({})).await;
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
    assert!(mcp.take_renewal_notices().is_empty());
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
        "loaded global MCP server: web (1 tools)"
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
        "loaded global MCP server: web (1 tools)"
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
            return HttpReply::json(json!({
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
    assert_eq!(
        mcp.call("mcp__web__echo", &json!({})).await,
        Some(Ok("pong".into()))
    );
    assert_eq!(requests_for_path(&requests, "/token").len(), 1);
}

#[tokio::test]
async fn asks_to_sign_in_when_an_http_server_needs_it() {
    let (url, _) = http_server(signed_in_mcp).await;
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "MCP server web needs sign-in: run /mcp login web"
    );
    let (url, _) = http_server(signed_in_mcp).await;
    let mut config = http_config(&url);
    config
        .headers
        .insert("authorization".into(), "Bearer wrong".into());
    assert_eq!(
        mcp.add(start("keyed".into(), Scope::Global, config).await)
            .status,
        "MCP server keyed failed: server answered initialize with status 401"
    );
}

/// Answers like a server that needs OAuth, with its sign-in server at
/// `/auth` on the same host, and records the forms sent to `/register`
/// and `/token`.
fn oauth_mcp(base: &str, request: &HttpRequest) -> HttpReply {
    let url = format!("{base}/mcp");
    match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/.well-known/oauth-protected-resource/mcp") => HttpReply::json(json!({
                "resource": url,
                "authorization_servers": [format!("{base}/auth")],
                "scopes_supported": ["unused"],
            })),
            ("GET", "/.well-known/oauth-authorization-server/auth") => HttpReply::json(json!({
                "issuer": format!("{base}/auth"),
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            })),
            ("POST", "/register") if request.body["client_name"] == "Claude Code" => {
                HttpReply::json(json!({ "client_id": "registered", "client_secret": "shh" }))
            }
            ("POST", "/register") => HttpReply::status(403, "Forbidden"),
            ("POST", "/token") if request.form().get("code").map(String::as_str) == Some("the-code") => {
                HttpReply::json(json!({
                    "access_token": "issued",
                    "refresh_token": "r",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                }))
            }
            ("POST", "/token") => HttpReply::status(400, r#"{"error":"invalid_grant"}"#),
            _ if request.headers.get("authorization").map(String::as_str)
                == Some("Bearer issued") =>
            {
                http_mcp(request)
            }
            _ => HttpReply::status(401, "").with_header(
                "www-authenticate",
                &format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp", scope="mcp:connect""#
                ),
            ),
        }
}

async fn oauth_server() -> (String, Requests) {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, requests) =
        http_server(move |request| oauth_mcp(&shared.lock().unwrap(), request)).await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    (url, requests)
}

fn query(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// Opens the redirect the sign-in server would send the browser to.
async fn visit_callback(authorize_url: &str, code: &str, state: Option<&str>) -> String {
    let parameters = query(authorize_url);
    let mut callback = reqwest::Url::parse(&parameters["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", state.unwrap_or(&parameters["state"]));
    reqwest::get(callback).await.unwrap().text().await.unwrap()
}

#[tokio::test]
async fn signs_in_to_an_http_server_with_oauth() {
    use base64::Engine;
    let _file = AUTH_FILE.read().await;
    let (url, requests) = oauth_server().await;
    let base = url.trim_end_matches("/mcp");
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let parameters = query(&sign_in.authorize_url);
    assert!(
        sign_in
            .authorize_url
            .starts_with(&format!("{base}/authorize?"))
    );
    assert_eq!(parameters["response_type"], "code");
    assert_eq!(parameters["client_id"], "registered");
    assert_eq!(parameters["code_challenge_method"], "S256");
    assert_eq!(parameters["scope"], "mcp:connect");
    assert_eq!(parameters["resource"], url);
    assert!(parameters["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert!(parameters["redirect_uri"].ends_with("/callback"));
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    let page = visit_callback(&authorize_url, "the-code", None).await;
    assert!(page.contains("Signed in"), "{page}");
    assert_eq!(finished.await.unwrap(), Ok(()));
    let registration = &requests_for_path(&requests, "/register")[0];
    assert_eq!(
        registration.body["redirect_uris"],
        json!([parameters["redirect_uri"]])
    );
    let token_form = requests_for_path(&requests, "/token")[0].form();
    assert_eq!(token_form["grant_type"], "authorization_code");
    assert_eq!(token_form["client_id"], "registered");
    assert_eq!(token_form["client_secret"], "shh");
    assert_eq!(token_form["redirect_uri"], parameters["redirect_uri"]);
    assert_eq!(token_form["resource"], url);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        <sha2::Sha256 as sha2::Digest>::digest(token_form["code_verifier"].as_bytes()),
    );
    assert_eq!(parameters["code_challenge"], challenge);
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "loaded global MCP server: web (1 tools)"
    );
}

#[tokio::test]
async fn signs_in_while_another_connection_to_the_callback_stays_idle() {
    let _file = AUTH_FILE.read().await;
    let (url, _) = oauth_server().await;
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let callback = reqwest::Url::parse(&query(&authorize_url)["redirect_uri"]).unwrap();
    let _idle = tokio::net::TcpStream::connect(("127.0.0.1", callback.port().unwrap()))
        .await
        .unwrap();
    let finished = tokio::spawn(sign_in.finish());
    let page = tokio::time::timeout(
        Duration::from_secs(10),
        visit_callback(&authorize_url, "the-code", None),
    )
    .await
    .expect("the idle connection stalled the sign-in");
    assert!(page.contains("Signed in"), "{page}");
    assert_eq!(finished.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn reads_sign_in_parameters_outside_quoted_values() {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
            let base = shared.lock().unwrap().clone();
            let reply = oauth_mcp(&base, request);
            if reply.status != 401 {
                return reply;
            }
            HttpReply::status(401, "").with_header(
                "www-authenticate",
                &format!(
                    r#"Bearer error="insufficient_scope", error_description="needs scope=admin, see resource_metadata=https://elsewhere.example", resource_metadata="{base}/.well-known/oauth-protected-resource/mcp", scope="mcp:connect""#
                ),
            )
        })
        .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "mcp:connect");
}

#[tokio::test]
async fn reads_sign_in_parameters_whatever_their_case() {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        let reply = oauth_mcp(&base, request);
        if reply.status != 401 {
            return reply;
        }
        HttpReply::status(401, "").with_header(
            "www-authenticate",
            &format!(
                r#"Bearer Resource_Metadata="{base}/.well-known/oauth-protected-resource/mcp", SCOPE="mcp:connect""#
            ),
        )
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "mcp:connect");
}

#[tokio::test]
async fn asks_for_the_scopes_the_server_lists_when_the_challenge_names_none() {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        let reply = oauth_mcp(&base, request);
        if request.path == "/.well-known/oauth-protected-resource/mcp" {
            return HttpReply::json(json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [format!("{base}/auth")],
                "scopes_supported": ["read", "write"],
            }));
        }
        if reply.status != 401 {
            return reply;
        }
        HttpReply::status(401, "").with_header(
            "www-authenticate",
            &format!(
                r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#
            ),
        )
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "read write");
}

#[tokio::test]
async fn refuses_sign_in_endpoints_without_https() {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        if request.path == "/.well-known/oauth-authorization-server/auth" {
            return HttpReply::json(json!({
                "authorization_endpoint": "https://sign-in.example/authorize",
                "token_endpoint": "http://sign-in.example/token",
                "registration_endpoint": format!("{base}/register"),
            }));
        }
        oauth_mcp(&base, request)
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(
            "the sign-in server's token endpoint does not use HTTPS: http://sign-in.example/token"
                .into()
        )
    );
}

#[tokio::test]
async fn refuses_to_sign_in_to_a_server_without_https() {
    assert_eq!(
        begin_sign_in_with("web", &http_config("http://mcp.example/mcp"))
            .await
            .err(),
        Some("the MCP server does not use HTTPS: http://mcp.example/mcp".into())
    );
}

/// Serves `oauth_mcp`, with `change` applied to the JSON it sends for
/// `path`.
async fn oauth_server_changing(
    path: &'static str,
    change: impl Fn(&mut Value) + Send + Sync + 'static,
) -> String {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        let mut reply = oauth_mcp(&base, request);
        if request.path == path {
            let mut metadata: Value = serde_json::from_str(&reply.body).unwrap();
            change(&mut metadata);
            reply = HttpReply::json(metadata);
        }
        reply
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    url
}

#[tokio::test]
async fn asks_for_a_client_id_when_the_sign_in_server_does_not_allow_registering() {
    let url = oauth_server_changing("/.well-known/oauth-authorization-server/auth", |metadata| {
        metadata
            .as_object_mut()
            .unwrap()
            .remove("registration_endpoint");
    })
    .await;
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(
            "the sign-in server does not let rust-coding-agent register; set oauth.clientId in mcp.json"
                .into()
        )
    );
}

#[tokio::test]
async fn reports_a_registration_the_sign_in_server_turned_down() {
    let url = oauth_server_changing("/.well-known/oauth-authorization-server/auth", |metadata| {
        let closed = metadata["registration_endpoint"]
            .as_str()
            .unwrap()
            .to_string()
            + "-closed";
        metadata["registration_endpoint"] = json!(closed);
    })
    .await;
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some("cannot register Claude Code with the sign-in server: status 401: ".into())
    );
}

#[tokio::test]
async fn refuses_resource_metadata_for_another_server() {
    let url = oauth_server_changing("/.well-known/oauth-protected-resource/mcp", |metadata| {
        metadata["resource"] = json!("https://other.example/mcp");
    })
    .await;
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(format!(
            "the sign-in metadata of {url} is for another server: https://other.example/mcp"
        ))
    );
}

#[tokio::test]
async fn refuses_resource_metadata_for_a_path_that_only_shares_a_prefix() {
    let url = oauth_server_changing("/.well-known/oauth-protected-resource/mcp", |metadata| {
        let resource = metadata["resource"].as_str().unwrap().trim_end_matches('p');
        metadata["resource"] = json!(resource);
    })
    .await;
    let resource = url.trim_end_matches('p');
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(format!(
            "the sign-in metadata of {url} is for another server: {resource}"
        ))
    );
}

#[tokio::test]
async fn accepts_resource_metadata_for_the_server_origin() {
    let url = oauth_server_changing("/.well-known/oauth-protected-resource/mcp", |metadata| {
        let resource = metadata["resource"]
            .as_str()
            .unwrap()
            .trim_end_matches("/mcp")
            .to_string();
        metadata["resource"] = json!(resource);
    })
    .await;
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    assert_eq!(
        query(&sign_in.authorize_url)["resource"],
        url.trim_end_matches("/mcp")
    );
}

#[tokio::test]
async fn refuses_a_sign_in_server_that_names_another_issuer() {
    let url = oauth_server_changing("/.well-known/oauth-authorization-server/auth", |metadata| {
        metadata["issuer"] = json!("https://other.example");
    })
    .await;
    let base = url.trim_end_matches("/mcp");
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(format!(
            "the sign-in server {base}/auth names another issuer: https://other.example"
        ))
    );
}

fn requests_for_path(requests: &Requests, path: &str) -> Vec<HttpRequest> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.path == path)
        .cloned()
        .collect()
}

#[tokio::test]
async fn signs_in_with_a_client_id_from_the_config() {
    let (url, requests) = oauth_server().await;
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("own".into()),
        ..OAuthConfig::default()
    });
    let sign_in = begin_sign_in_with("web", &config).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["client_id"], "own");
    assert!(requests_for_path(&requests, "/register").is_empty());
}

#[tokio::test]
async fn does_not_save_the_client_secret_from_the_config() {
    let _file = AUTH_FILE.read().await;
    let (url, requests) = oauth_server().await;
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("own".into()),
        client_secret: Some("own-secret".into()),
    });
    let sign_in = begin_sign_in_with("web", &config).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    visit_callback(&authorize_url, "the-code", None).await;
    assert_eq!(finished.await.unwrap(), Ok(()));
    assert_eq!(
        requests_for_path(&requests, "/token")[0].form()["client_secret"],
        "own-secret"
    );
    let file = std::fs::read_to_string(config::dir().unwrap().join("mcp-auth.json")).unwrap();
    assert!(!file.contains("own-secret"), "{file}");
}

#[tokio::test]
async fn reports_failed_sign_ins() {
    let (url, _) = oauth_server().await;
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    let page = visit_callback(&authorize_url, "the-code", Some("forged")).await;
    assert!(page.contains("Sign-in failed"), "{page}");
    assert_eq!(
        finished.await.unwrap(),
        Err("the sign-in page sent back a different state".into())
    );
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    visit_callback(&authorize_url, "wrong-code", None).await;
    assert_eq!(
        finished.await.unwrap(),
        Err("token endpoint answered with status 400: invalid_grant".into())
    );
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let mut callback = reqwest::Url::parse(&query(&sign_in.authorize_url)["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("state", "forged");
    let finished = tokio::spawn(sign_in.finish());
    reqwest::get(callback).await.unwrap();
    assert_eq!(
        finished.await.unwrap(),
        Err("the sign-in page sent back a different state".into())
    );
    let mut keyed = http_config(&url);
    keyed
        .headers
        .insert("Authorization".into(), "Bearer key".into());
    assert_eq!(
        begin_sign_in_with("keyed", &keyed).await.err(),
        Some("MCP server keyed sends its own Authorization header".into())
    );
    assert_eq!(
        begin_sign_in_with("local", &echo_server()).await.err(),
        Some("MCP server local is not an HTTP server".into())
    );
}

#[tokio::test]
async fn reports_a_sign_in_the_user_declined() {
    let (url, _) = oauth_server().await;
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let parameters = query(&sign_in.authorize_url);
    let mut callback = reqwest::Url::parse(&parameters["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("error_description", "the user said no")
        .append_pair("state", &parameters["state"]);
    let finished = tokio::spawn(sign_in.finish());
    let other = callback.join("/favicon.ico").unwrap();
    assert_eq!(reqwest::get(other).await.unwrap().status(), 404);
    let page = reqwest::get(callback).await.unwrap().text().await.unwrap();
    assert!(
        page.contains("Sign-in failed: access_denied: the user said no"),
        "{page}"
    );
    assert_eq!(
        finished.await.unwrap(),
        Err("access_denied: the user said no".into())
    );
}

#[tokio::test]
async fn signs_out_of_an_http_server() {
    let _file = AUTH_FILE.read().await;
    let (url, _) = http_server(signed_in_mcp).await;
    save_tokens(&url, "fresh", now_millis() + 3_600_000);
    assert_eq!(
        sign_out_with("web", &http_config(&url)),
        Ok("signed out of MCP server web".into())
    );
    assert!(oauth::load(&url).is_none());
    assert_eq!(
        sign_out_with("web", &http_config(&url)),
        Ok("MCP server web was not signed in".into())
    );
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
            .status,
        "MCP server web needs sign-in: run /mcp login web"
    );
    assert_eq!(
        sign_out_with("local", &echo_server()),
        Err("MCP server local is not an HTTP server".into())
    );
}
