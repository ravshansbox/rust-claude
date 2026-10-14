use super::*;

async fn token_after_renewal_with(reply: Value) -> Result<String, String> {
    answer_token_requests("200 OK", reply).await;
    let path = no_credentials_file();
    let http = reqwest::Client::new();
    let mut credentials = expired("first");
    let first = credentials.access_token(&http).await;
    answer_overloaded().await;
    let second = credentials.access_token(&http).await;
    let _ = std::fs::remove_file(&path);
    assert_eq!(first.unwrap(), "new");
    second.map_err(|error| format!("{error:#}"))
}

#[tokio::test]
async fn uses_a_short_lived_token_without_renewing_it_again() {
    let _lock = AUTH_FILE.lock().await;
    let token = token_after_renewal_with(json!({
        "access_token": "new",
        "refresh_token": "second",
        "expires_in": 60,
    }))
    .await;
    assert_eq!(token, Ok("new".into()));
}

#[tokio::test]
async fn uses_a_token_without_expiry_without_renewing_it_again() {
    let _lock = AUTH_FILE.lock().await;
    let token = token_after_renewal_with(json!({
        "access_token": "new",
        "refresh_token": "second",
    }))
    .await;
    assert_eq!(token, Ok("new".into()));
}

#[tokio::test]
async fn keeps_the_refresh_token_when_a_renewal_does_not_replace_it() {
    let _lock = AUTH_FILE.lock().await;
    answer_token_requests(
        "200 OK",
        json!({ "access_token": "new", "expires_in": 3600 }),
    )
    .await;
    let path = no_credentials_file();
    let mut credentials = expired("first");
    let token = credentials.access_token(&reqwest::Client::new()).await;
    let saved = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(token.unwrap(), "new");
    let saved: Credentials = serde_json::from_str(&saved.unwrap()).unwrap();
    assert_eq!(saved.refresh, "first");
}
