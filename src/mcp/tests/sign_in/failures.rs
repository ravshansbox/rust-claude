use super::*;

#[tokio::test]
async fn reports_failed_sign_ins() {
    let (url, _) = oauth_server().await;
    let sign_in = begin_web_sign_in(&url).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    let page = visit_callback(&authorize_url, "the-code", Some("forged")).await;
    assert!(page.contains("Sign-in failed"), "{page}");
    assert_eq!(
        finished.await.unwrap(),
        Err("the sign-in page sent back a different state".into())
    );
    let sign_in = begin_web_sign_in(&url).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    visit_callback(&authorize_url, "wrong-code", None).await;
    assert_eq!(
        finished.await.unwrap(),
        Err("token endpoint answered with status 400: invalid_grant".into())
    );
    let sign_in = begin_web_sign_in(&url).await.unwrap();
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
    let sign_in = begin_web_sign_in(&url).await.unwrap();
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
