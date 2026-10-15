use super::*;

/// Answers like a server that needs OAuth, with its sign-in server at
/// `/auth` on the same host, and records the forms sent to `/register`
/// and `/token`.
pub(super) fn oauth_mcp(base: &str, request: &HttpRequest) -> HttpReply {
    let url = format!("{base}/mcp");
    match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/.well-known/oauth-protected-resource/mcp") => HttpReply::json(&json!({
                "resource": url,
                "authorization_servers": [format!("{base}/auth")],
                "scopes_supported": ["unused"],
            })),
            ("GET", "/.well-known/oauth-authorization-server/auth") => HttpReply::json(&json!({
                "issuer": format!("{base}/auth"),
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            })),
            ("POST", "/register") if request.body["client_name"] == "Claude Code" => {
                HttpReply::json(&json!({ "client_id": "registered", "client_secret": "shh" }))
            }
            ("POST", "/register") => HttpReply::status(403, "Forbidden"),
            ("POST", "/token") if request.form().get("code").map(String::as_str) == Some("the-code") => {
                HttpReply::json(&json!({
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

pub(super) async fn oauth_server() -> (String, Requests) {
    oauth_server_with(oauth_mcp).await
}

/// Serves `handler`, which gets the server's base URL with each request.
pub(super) async fn oauth_server_with(
    handler: impl Fn(&str, &HttpRequest) -> HttpReply + Send + Sync + 'static,
) -> (String, Requests) {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, requests) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        handler(&base, request)
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    (url, requests)
}

pub(super) async fn begin_web_sign_in(url: &str) -> Result<SignIn, String> {
    begin_sign_in_with("web", &http_config(url)).await
}

pub(super) fn query(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// Opens the redirect the sign-in server would send the browser to.
pub(super) async fn visit_callback(authorize_url: &str, code: &str, state: Option<&str>) -> String {
    let parameters = query(authorize_url);
    let mut callback = reqwest::Url::parse(&parameters["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", state.unwrap_or(&parameters["state"]));
    reqwest::get(callback).await.unwrap().text().await.unwrap()
}

pub(super) fn requests_for_path(requests: &Requests, path: &str) -> Vec<HttpRequest> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.path == path)
        .cloned()
        .collect()
}
