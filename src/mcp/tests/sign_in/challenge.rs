use super::*;

#[tokio::test]
async fn reads_sign_in_parameters_outside_quoted_values() {
    let (url, _) = oauth_server_with(|base, request| {
        let reply = oauth_mcp(base, request);
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
    let sign_in = begin_web_sign_in(&url).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "mcp:connect");
}

#[tokio::test]
async fn reads_sign_in_parameters_whatever_their_case() {
    let (url, _) = oauth_server_with(|base, request| {
        let reply = oauth_mcp(base, request);
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
    let sign_in = begin_web_sign_in(&url).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "mcp:connect");
}

#[tokio::test]
async fn asks_for_the_scopes_the_server_lists_when_the_challenge_names_none() {
    let (url, _) = oauth_server_with(|base, request| {
        let reply = oauth_mcp(base, request);
        if request.path == "/.well-known/oauth-protected-resource/mcp" {
            return HttpReply::json(&json!({
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
    let sign_in = begin_web_sign_in(&url).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "read write");
}
