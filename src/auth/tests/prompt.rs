use super::*;

#[tokio::test]
async fn renews_a_sign_in_the_api_turned_down_and_sends_the_request_again() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("refresh", false).await;
    let path = no_credentials_file();
    let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
    let mut agent = test_support::agent(&api);
    let mut text = String::new();
    let result = agent.prompt("hi", &[], test_support::text(&mut text)).await;
    test_support::remove_session(&agent);
    let _ = std::fs::remove_file(&path);
    result.unwrap();
    assert_eq!(text, "hello");
    assert_eq!(*endpoint.received.lock().await, ["refresh"]);
    let headers = api.headers().await;
    assert_eq!(headers.len(), 2);
    assert!(
        headers[1].contains("authorization: bearer access from refresh"),
        "{}",
        headers[1]
    );
}

#[tokio::test]
async fn renews_an_expired_sign_in_before_a_prompt_and_says_so() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("refresh", false).await;
    let path = no_credentials_file();
    let api = MockApi::start(vec![text_reply("hello")]).await;
    let mut agent = crate::agent::Agent::new(
        reqwest::Client::new(),
        expired("refresh"),
        "claude-opus-5-5".into(),
    )
    .unwrap();
    agent.api_base = api.base.clone();
    let mut notices = Vec::new();
    let result = agent
        .prompt("hi", &[], test_support::notices(&mut notices))
        .await;
    test_support::remove_session(&agent);
    let _ = std::fs::remove_file(&path);
    result.unwrap();
    assert_eq!(notices, ["renewed sign-in token"]);
    assert_eq!(*endpoint.received.lock().await, ["refresh"]);
    let headers = api.headers().await;
    assert!(
        headers[0].contains("authorization: bearer access from refresh"),
        "{}",
        headers[0]
    );
}

#[tokio::test]
async fn uses_a_sign_in_another_instance_renewed_after_the_api_turned_ours_down() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("refresh", false).await;
    let path = credentials_file(
        &json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_001u64 })
            .to_string(),
    );
    let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
    let mut agent = test_support::agent(&api);
    let result = agent.prompt("hi", &[], |_| {}).await;
    test_support::remove_session(&agent);
    let _ = std::fs::remove_file(&path);
    result.unwrap();
    assert!(endpoint.received.lock().await.is_empty());
    let headers = api.headers().await;
    assert_eq!(headers.len(), 2);
    assert!(
        headers[1].contains("authorization: bearer renewed"),
        "{}",
        headers[1]
    );
}

#[tokio::test]
async fn asks_to_sign_in_again_when_a_sign_in_the_api_turned_down_cannot_be_renewed() {
    let _lock = AUTH_FILE.lock().await;
    let _endpoint = TokenEndpoint::start("other", false).await;
    let path = no_credentials_file();
    let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
    let mut agent = test_support::agent(&api);
    let result = agent.prompt("hi", &[], |_| {}).await;
    test_support::remove_session(&agent);
    // The next start reads what was saved and, unable to renew it, signs
    // in again instead of sending the turned-down token.
    let saved = std::fs::read_to_string(&path).unwrap_or_default();
    let next_start = Credentials::load_and_refresh(&saved, &reqwest::Client::new()).await;
    let _ = std::fs::remove_file(&path);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("sign in again"), "{error}");
    assert_eq!(api.requests().await.len(), 1);
    assert!(next_start.is_err_and(|error| error.is::<crate::auth::Unusable>()));
}
