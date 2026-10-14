use super::*;

#[tokio::test]
async fn refuses_sign_in_endpoints_without_https() {
    let (url, _) = oauth_server_with(|base, request| {
        if request.path == "/.well-known/oauth-authorization-server/auth" {
            return HttpReply::json(&json!({
                "authorization_endpoint": "https://sign-in.example/authorize",
                "token_endpoint": "http://sign-in.example/token",
                "registration_endpoint": format!("{base}/register"),
            }));
        }
        oauth_mcp(base, request)
    })
    .await;
    assert_eq!(
        begin_web_sign_in(&url).await.err(),
        Some(
            "the sign-in server's token endpoint does not use HTTPS: http://sign-in.example/token"
                .into()
        )
    );
}

#[tokio::test]
async fn refuses_to_sign_in_to_a_server_without_https() {
    assert_eq!(
        begin_web_sign_in("http://mcp.example/mcp").await.err(),
        Some("the MCP server does not use HTTPS: http://mcp.example/mcp".into())
    );
}

/// Serves `oauth_mcp`, with `change` applied to the JSON it sends for
/// `path`.
async fn oauth_server_changing(
    path: &'static str,
    change: impl Fn(&mut Value) + Send + Sync + 'static,
) -> String {
    let (url, _) = oauth_server_with(move |base, request| {
        let mut reply = oauth_mcp(base, request);
        if request.path == path {
            let mut metadata: Value = serde_json::from_str(&reply.body).unwrap();
            change(&mut metadata);
            reply = HttpReply::json(&metadata);
        }
        reply
    })
    .await;
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
        begin_web_sign_in(&url).await.err(),
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
        begin_web_sign_in(&url).await.err(),
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
        begin_web_sign_in(&url).await.err(),
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
        begin_web_sign_in(&url).await.err(),
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
    let sign_in = begin_web_sign_in(&url).await.unwrap();
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
        begin_web_sign_in(&url).await.err(),
        Some(format!(
            "the sign-in server {base}/auth names another issuer: https://other.example"
        ))
    );
}
