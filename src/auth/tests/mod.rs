use super::{Credentials, credentials_path, exchange_code};
use crate::agent::{
    retry::retry_delay,
    test_support::{self, MockApi, Reply, text_reply},
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
};

mod instances;
mod pasted_code;
mod prompt;
mod quitting;
mod renewed_tokens;

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
pub(crate) async fn answer_token_requests(status: &'static str, body: Value) {
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

pub(crate) fn no_credentials_file() -> PathBuf {
    let path = credentials_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&path);
    path
}

fn credentials_file(saved: &str) -> PathBuf {
    let path = credentials_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, saved).unwrap();
    path
}

pub(crate) fn expired(refresh: &str) -> Credentials {
    serde_json::from_value(json!({ "access": "old", "refresh": refresh, "expires": 0 })).unwrap()
}

fn renew_in_background() -> tokio::task::JoinHandle<anyhow::Result<String>> {
    tokio::spawn(async {
        let mut credentials = expired("first");
        credentials.access_token(&reqwest::Client::new()).await
    })
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
