use super::{Credentials, credentials_path, exchange_code, finish_renewal, parse_pasted_code};
use crate::agent::{
    AgentEvent,
    retry::retry_delay,
    test_support::{self, MockApi, Reply, text_reply},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
};

/// Tests share one auth.json, so they take turns.
pub(crate) static AUTH_FILE: Mutex<()> = Mutex::const_new(());

/// Where the running test answers token requests.
pub(super) static TOKEN_URL: Mutex<Option<String>> = Mutex::const_new(None);

fn unreachable_client() -> reqwest::Client {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
        .build()
        .unwrap()
}

/// A local stand-in for the token endpoint. Like the real one, it accepts
/// only the refresh token it issued last, once, and answers with a new
/// pair.
struct TokenEndpoint {
    /// The refresh tokens sent to it.
    received: Arc<Mutex<Vec<String>>>,
    /// Signalled when a request arrives.
    arrived: Arc<Notify>,
    /// Lets a held reply go, when the endpoint is slow.
    release: Arc<Notify>,
}

impl TokenEndpoint {
    async fn start(refresh: &str, slow: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        *TOKEN_URL.lock().await = Some(format!(
            "http://{}/v1/oauth/token",
            listener.local_addr().unwrap()
        ));
        let endpoint = Self {
            received: Arc::default(),
            arrived: Arc::default(),
            release: Arc::default(),
        };
        let valid = Arc::new(Mutex::new(refresh.to_string()));
        let (received, arrived, release) = (
            endpoint.received.clone(),
            endpoint.arrived.clone(),
            endpoint.release.clone(),
        );
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let (received, arrived, release, valid) = (
                    received.clone(),
                    arrived.clone(),
                    release.clone(),
                    valid.clone(),
                );
                tokio::spawn(async move {
                    let body = read_body(&mut stream).await;
                    let token = body["refresh_token"].as_str().unwrap_or_default();
                    received.lock().await.push(token.to_string());
                    arrived.notify_one();
                    if slow {
                        release.notified().await;
                    }
                    let reply = {
                        let mut valid = valid.lock().await;
                        (token == *valid).then(|| {
                            *valid = format!("{token}+");
                            json!({
                                "access_token": format!("access from {token}"),
                                "refresh_token": *valid,
                                "expires_in": 3600,
                            })
                        })
                    };
                    let (status, body) = match reply {
                        Some(body) => ("200 OK", body),
                        None => ("400 Bad Request", json!({ "error": "invalid_grant" })),
                    };
                    respond(&mut stream, status, &body).await;
                });
            }
        });
        endpoint
    }
}

/// A token endpoint that gives every request the same answer.
async fn answer_token_requests(status: &'static str, body: Value) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    *TOKEN_URL.lock().await = Some(format!(
        "http://{}/v1/oauth/token",
        listener.local_addr().unwrap()
    ));
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                read_body(&mut stream).await;
                respond(&mut stream, status, &body).await;
            });
        }
    });
}

async fn answer_overloaded() {
    answer_token_requests("503 Service Unavailable", json!({ "error": "overloaded" })).await;
}

async fn respond(stream: &mut TcpStream, status: &str, body: &Value) {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn read_body(stream: &mut TcpStream) -> Value {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&data[..end]).to_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                return serde_json::from_slice(&data[end + 4..]).unwrap_or(Value::Null);
            }
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return Value::Null,
            Ok(read) => data.extend_from_slice(&chunk[..read]),
        }
    }
}

fn no_credentials_file() -> std::path::PathBuf {
    let path = credentials_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&path);
    path
}

fn credentials_file(saved: &str) -> std::path::PathBuf {
    let path = credentials_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, saved).unwrap();
    path
}

fn expired(refresh: &str) -> Credentials {
    serde_json::from_value(json!({ "access": "old", "refresh": refresh, "expires": 0 })).unwrap()
}

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

#[tokio::test]
async fn finishes_renewing_after_the_caller_gives_up() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", true).await;
    let path = no_credentials_file();
    let prompt = tokio::spawn(async {
        let mut credentials = expired("first");
        credentials.access_token(&reqwest::Client::new()).await
    });
    endpoint.arrived.notified().await;
    prompt.abort();
    let _ = prompt.await;
    endpoint.release.notify_one();
    let mut saved = None;
    for _ in 0..100 {
        saved = std::fs::read_to_string(&path).ok();
        if saved.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = std::fs::remove_file(&path);
    let saved: Credentials = serde_json::from_str(&saved.expect("auth.json not saved")).unwrap();
    assert_eq!(
        (saved.access.as_str(), saved.refresh.as_str()),
        ("access from first", "first+")
    );
}

#[tokio::test]
async fn quitting_waits_for_a_renewal_to_be_saved() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", true).await;
    let path = no_credentials_file();
    let prompt = tokio::spawn(async {
        let mut credentials = expired("first");
        credentials.access_token(&reqwest::Client::new()).await
    });
    endpoint.arrived.notified().await;
    prompt.abort();
    let _ = prompt.await;
    let release = endpoint.release.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.notify_one();
    });
    finish_renewal(Duration::from_secs(5)).await;
    let saved = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let saved: Credentials = serde_json::from_str(&saved.expect("auth.json not saved")).unwrap();
    assert_eq!(saved.refresh, "first+");
}

#[tokio::test]
async fn quitting_stops_waiting_for_a_renewal_that_hangs() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", true).await;
    let path = no_credentials_file();
    let prompt = tokio::spawn(async {
        let mut credentials = expired("first");
        credentials.access_token(&reqwest::Client::new()).await
    });
    endpoint.arrived.notified().await;
    let waited = tokio::time::timeout(
        Duration::from_secs(5),
        finish_renewal(Duration::from_millis(100)),
    )
    .await;
    endpoint.release.notify_one();
    let _ = prompt.await;
    let _ = std::fs::remove_file(&path);
    assert!(waited.is_ok(), "kept waiting for the renewal");
}

#[tokio::test]
async fn renews_a_sign_in_the_api_turned_down_and_sends_the_request_again() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("refresh", false).await;
    let path = no_credentials_file();
    let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    let mut text = String::new();
    let result = agent
        .prompt("hi", &[], |event| {
            if let AgentEvent::Text(delta) = event {
                text.push_str(&delta);
            }
        })
        .await;
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
async fn uses_a_sign_in_another_instance_renewed_after_the_api_turned_ours_down() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("refresh", false).await;
    let path = credentials_file(
        &json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_001u64 })
            .to_string(),
    );
    let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
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
    let mut agent = test_support::agent(&api, reqwest::Client::new());
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
    assert!(next_start.is_err_and(|error| error.is::<super::Unusable>()));
}

#[tokio::test]
async fn running_copies_renew_one_at_a_time() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", false).await;
    let path =
        credentials_file(&json!({ "access": "old", "refresh": "first", "expires": 0 }).to_string());
    let http = reqwest::Client::new();
    let (mut one, mut other) = (expired("first"), expired("first"));
    let (token, other_token) = tokio::join!(one.access_token(&http), other.access_token(&http));
    let _ = std::fs::remove_file(&path);
    assert_eq!(*endpoint.received.lock().await, ["first"]);
    assert_eq!(token.unwrap(), "access from first");
    assert_eq!(other_token.unwrap(), "access from first");
}

#[tokio::test]
async fn uses_a_sign_in_another_instance_renewed_after_startup() {
    let _lock = AUTH_FILE.lock().await;
    let path = credentials_file(
        &json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_000u64 })
            .to_string(),
    );
    let read_at_startup = json!({ "access": "old", "refresh": "spent", "expires": 0 }).to_string();
    let credentials = Credentials::load_and_refresh(&read_at_startup, &unreachable_client()).await;
    let _ = std::fs::remove_file(&path);
    assert_eq!(credentials.unwrap().access, "renewed");
}

#[tokio::test]
async fn uses_a_sign_in_without_expiry_another_instance_renewed() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("spent", false).await;
    let renewed = json!({ "access": "renewed", "refresh": "new", "expires": u64::MAX });
    let path = credentials_file(&renewed.to_string());
    let mut credentials: Credentials =
        serde_json::from_value(json!({ "access": "old", "refresh": "spent", "expires": u64::MAX }))
            .unwrap();
    let token = credentials
        .renew_rejected(&reqwest::Client::new(), "old")
        .await;
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(token.unwrap(), "renewed");
    assert!(endpoint.received.lock().await.is_empty());
    assert_eq!(saved, renewed);
}

#[tokio::test]
async fn saving_never_leaves_a_reader_with_a_partial_sign_in() {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    let _lock = AUTH_FILE.lock().await;
    let old = json!({ "access": "old", "refresh": "old", "expires": 1 }).to_string();
    let path = credentials_file(&old);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut reader = std::fs::File::open(&path).unwrap();

    let new: Credentials =
        serde_json::from_value(json!({ "access": "new", "refresh": "new", "expires": 2 })).unwrap();
    new.save().unwrap();

    let mut seen_by_reader = String::new();
    reader.read_to_string(&mut seen_by_reader).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let saved: Credentials =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let leftovers = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name.to_string_lossy().starts_with("auth.json.")
        })
        .count();
    let _ = std::fs::remove_file(&path);
    assert_eq!(seen_by_reader, old);
    assert_eq!(mode, 0o600);
    assert_eq!((saved.access.as_str(), saved.expires), ("new", 2));
    assert_eq!(leftovers, 0);
}

#[test]
fn accepts_a_pasted_code_with_the_state_we_sent() {
    assert_eq!(parse_pasted_code(" abc#xyz\n", "xyz").unwrap(), "abc");
}

#[test]
fn rejects_a_pasted_code_with_another_state() {
    assert_eq!(
        parse_pasted_code("abc#other", "xyz")
            .unwrap_err()
            .to_string(),
        "OAuth state mismatch"
    );
}

#[test]
fn rejects_a_pasted_code_without_its_state() {
    let error = parse_pasted_code("abc\n", "xyz").unwrap_err();
    assert!(error.to_string().contains("whole code"), "{error}");
}
