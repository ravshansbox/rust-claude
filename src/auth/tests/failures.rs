use super::*;

#[tokio::test]
async fn keeps_the_saved_sign_in_when_the_token_endpoint_is_down() {
    let _lock = AUTH_FILE.lock().await;
    answer_overloaded().await;
    let saved = json!({ "access": "old", "refresh": "first", "expires": 0 }).to_string();
    let path = credentials_file(&saved);
    let result = Credentials::load_or_login(&reqwest::Client::new()).await;
    let after = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let error = format!(
        "{:#}",
        result.err().expect("signed in without the endpoint")
    );
    assert!(error.contains("503"), "{error}");
    assert_eq!(after, saved);
}

#[tokio::test]
async fn retries_a_renewal_the_server_could_not_answer() {
    let _lock = AUTH_FILE.lock().await;
    answer_overloaded().await;
    let _ = std::fs::remove_file(credentials_path().unwrap());
    let mut credentials = expired("first");
    let error = credentials
        .access_token(&reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(retry_delay(&error, 0).is_some(), "{error:#}");
    assert!(error.to_string().contains("503"), "{error}");
}

#[tokio::test]
async fn keeps_a_turned_down_sign_in_when_renewing_is_rate_limited_or_times_out() {
    let _lock = AUTH_FILE.lock().await;
    for status in ["429 Too Many Requests", "408 Request Timeout"] {
        answer_token_requests(status, json!({ "error": "slow down" })).await;
        let saved = json!({ "access": "old", "refresh": "first", "expires": u64::MAX });
        let path = credentials_file(&saved.to_string());
        let mut credentials: Credentials = serde_json::from_value(saved.clone()).unwrap();
        let result = credentials
            .renew_rejected(&reqwest::Client::new(), "old")
            .await;
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let error = result.unwrap_err();
        assert!(retry_delay(&error, 0).is_some(), "{status}: {error:#}");
        assert_eq!(after, saved, "{status}");
    }
}

#[tokio::test]
async fn asks_to_sign_in_again_when_the_server_turns_the_renewal_down() {
    let _lock = AUTH_FILE.lock().await;
    let _endpoint = TokenEndpoint::start("other", false).await;
    let _ = std::fs::remove_file(credentials_path().unwrap());
    let mut credentials = expired("spent");
    let error = credentials
        .access_token(&reqwest::Client::new())
        .await
        .unwrap_err();
    assert_eq!(retry_delay(&error, 0), None);
    assert!(error.to_string().contains("sign in again"), "{error}");
}

#[tokio::test]
async fn keeps_a_new_sign_in_that_could_not_be_saved() {
    let _lock = AUTH_FILE.lock().await;
    answer_token_requests(
        "200 OK",
        json!({ "access_token": "new", "refresh_token": "next", "expires_in": 3600 }),
    )
    .await;
    let path = credentials_path().unwrap();
    // A folder in the way of auth.json makes saving fail.
    std::fs::create_dir_all(&path).unwrap();
    let credentials = exchange_code(&reqwest::Client::new(), "code", "state", "verifier").await;
    let _ = std::fs::remove_dir_all(&path);
    assert_eq!(credentials.unwrap().access, "new");
}

#[tokio::test]
async fn keeps_a_renewed_sign_in_that_could_not_be_saved() {
    let _lock = AUTH_FILE.lock().await;
    let _endpoint = TokenEndpoint::start("first", false).await;
    let path = credentials_path().unwrap();
    // A folder in the way of auth.json makes saving fail.
    std::fs::create_dir_all(&path).unwrap();
    let mut credentials = expired("first");
    let token = credentials.access_token(&reqwest::Client::new()).await;
    let notice = credentials.take_renewal_notice();
    let _ = std::fs::remove_dir_all(&path);
    assert_eq!(token.unwrap(), "access from first");
    let notice = notice.unwrap();
    assert!(notice.contains("could not save"), "{notice}");
}
