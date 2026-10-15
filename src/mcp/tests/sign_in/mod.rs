use super::*;

mod challenge;
mod client;
mod failures;
mod metadata;
mod oauth_server;
mod renewal;
mod user;

use oauth_server::*;

/// Tests share one mcp-auth.json: those that use it take turns with those
/// that make it unusable.
pub(crate) static AUTH_FILE: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Answers like `http_mcp`, but only with the access token `fresh`, and
/// renews the refresh token `r1` at `/token`.
fn signed_in_mcp(request: &HttpRequest) -> HttpReply {
    if request.path == "/token" {
        let form = request.form();
        if form.get("grant_type").map(String::as_str) == Some("refresh_token")
            && form.get("refresh_token").map(String::as_str) == Some("r1")
            && form.get("client_id").map(String::as_str) == Some("client")
        {
            return HttpReply::json(&json!({
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

pub(crate) fn save_tokens(url: &str, access_token: &str, expires_at: u128) {
    save_tokens_as(url, access_token, expires_at, None);
}

pub(crate) fn save_tokens_as(url: &str, access_token: &str, expires_at: u128, user: Option<&str>) {
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
            user: user.map(str::to_string),
        },
    )
    .unwrap();
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

#[tokio::test]
async fn signs_in_to_an_http_server_with_oauth() {
    use base64::Engine;
    let _file = AUTH_FILE.read().await;
    let (url, requests) = oauth_server().await;
    let base = url.trim_end_matches("/mcp");
    let sign_in = begin_web_sign_in(&url).await.unwrap();
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
    assert!(
        page.contains("Signed in. You can close this tab and go back to Claude Code."),
        "{page}"
    );
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
    let added = mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert_eq!(added.status, "loaded global MCP server: web (1 tool)");
    assert_eq!(added.summary, "web (signed in, 1 tool)");
}

#[tokio::test]
async fn signs_in_while_another_connection_to_the_callback_stays_idle() {
    let _file = AUTH_FILE.read().await;
    let (url, _) = oauth_server().await;
    let sign_in = begin_web_sign_in(&url).await.unwrap();
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
