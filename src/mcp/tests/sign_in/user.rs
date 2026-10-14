use super::*;

fn jwt(claims: &Value) -> String {
    use base64::Engine;
    let encode =
        |value: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    format!(
        "{}.{}.signature",
        encode(&json!({ "alg": "RS256" })),
        encode(claims)
    )
}

/// Signs in to a server whose token endpoint adds `tokens` to its reply,
/// and returns the summary of the server started with that sign-in.
async fn summary_after_sign_in(tokens: Value) -> String {
    let bearer = format!(
        "Bearer {}",
        tokens["access_token"].as_str().unwrap_or("issued")
    );
    let (url, _) = oauth_server_with(move |base, request| {
        if request.headers.get("authorization") == Some(&bearer) {
            return http_mcp(request);
        }
        let mut reply = oauth_mcp(base, request);
        if request.path == "/token" && reply.status == 200 {
            let mut body: Value = serde_json::from_str(&reply.body).unwrap();
            for (key, value) in tokens.as_object().unwrap() {
                body[key] = value.clone();
            }
            reply = HttpReply::json(&body);
        }
        reply
    })
    .await;
    let sign_in = begin_web_sign_in(&url).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    visit_callback(&authorize_url, "the-code", None).await;
    assert_eq!(finished.await.unwrap(), Ok(()));
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
        .summary
}

#[tokio::test]
async fn hides_the_user_in_the_id_token_from_the_summary() {
    let _file = AUTH_FILE.read().await;
    let id_token = jwt(&json!({
        "sub": "123",
        "email": "alice@example.com",
        "preferred_username": "alice",
    }));
    assert_eq!(
        summary_after_sign_in(json!({ "id_token": id_token })).await,
        "web (signed in, 1 tool)"
    );
}

#[tokio::test]
async fn hides_the_user_in_the_access_token_from_the_summary() {
    let _file = AUTH_FILE.read().await;
    let access_token = jwt(&json!({ "sub": "123", "email": "alice@example.com" }));
    let summary = summary_after_sign_in(json!({ "access_token": access_token })).await;
    assert_eq!(summary, "web (signed in, 1 tool)");
}

#[tokio::test]
async fn keeps_the_user_when_renewed_tokens_do_not_name_one() {
    let _file = AUTH_FILE.read().await;
    let (url, _) = http_server(signed_in_mcp).await;
    save_tokens_as(&url, "stale", now_millis() - 1, Some("alice"));
    let mut mcp = Mcp::default();
    let added = mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert_eq!(added.summary, "web (signed in, 1 tool)");
    assert_eq!(oauth::load(&url).unwrap().user.as_deref(), Some("alice"));
}
