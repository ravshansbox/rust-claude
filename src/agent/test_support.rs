use super::Agent;
use crate::auth::Credentials;
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

pub(crate) enum Reply {
    Stall,
    Events(Vec<Value>),
    /// A 400 `invalid_request_error` with this message.
    BadRequest(String),
}

pub(crate) fn text_reply(text: &str) -> Reply {
    stopped_reply(text, "end_turn")
}

/// A text reply that ends with `stop_reason`.
pub(crate) fn stopped_reply(text: &str, stop_reason: &str) -> Reply {
    Reply::Events(vec![
        json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "message_delta", "delta": { "stop_reason": stop_reason }, "usage": { "output_tokens": 1 } }),
        json!({ "type": "message_stop" }),
    ])
}

pub(crate) fn tool_reply(id: &str, name: &str, input: Value) -> Reply {
    Reply::Events(vec![
        json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": id, "name": name, "input": input } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }),
        json!({ "type": "message_stop" }),
    ])
}

/// A local stand-in for the Messages API. It answers each request with the
/// next scripted reply and records the request headers and bodies it received.
pub(crate) struct MockApi {
    pub base: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
}

impl MockApi {
    pub(crate) async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut replies = VecDeque::from(replies);
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let Some(request) = read_request(&mut stream).await else {
                    continue;
                };
                recorded.lock().await.push(request);
                tokio::spawn(respond(stream, replies.pop_front()));
            }
        });
        Self { base, requests }
    }

    pub(crate) async fn requests(&self) -> Vec<Value> {
        let requests = self.requests.lock().await;
        requests.iter().map(|(_, body)| body.clone()).collect()
    }

    /// The headers of each request, lowercased.
    pub(crate) async fn headers(&self) -> Vec<String> {
        let requests = self.requests.lock().await;
        requests
            .iter()
            .map(|(headers, _)| headers.clone())
            .collect()
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<(String, Value)> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while data.len() < header_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&data[header_end..]).unwrap_or(Value::Null);
    Some((headers, body))
}

async fn respond(mut stream: TcpStream, reply: Option<Reply>) {
    match reply {
        Some(Reply::Events(events)) => {
            let body: String = events
                .iter()
                .map(|event| {
                    format!(
                        "event: {}\ndata: {event}\n\n",
                        event["type"].as_str().unwrap()
                    )
                })
                .collect();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
        Some(Reply::Stall) => {
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                .await;
            std::future::pending::<()>().await;
        }
        Some(Reply::BadRequest(message)) => bad_request(stream, &message).await,
        None => bad_request(stream, "no scripted reply").await,
    }
}

async fn bad_request(mut stream: TcpStream, message: &str) {
    let body = json!({
        "type": "error",
        "error": { "type": "invalid_request_error", "message": message },
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

pub(crate) fn agent(api: &MockApi, http: reqwest::Client) -> Agent {
    let credentials: Credentials = serde_json::from_value(json!({
        "access": "access",
        "refresh": "refresh",
        "expires": 4_102_444_800_000u64,
    }))
    .unwrap();
    let mut agent = Agent::new(http, credentials, "claude-opus-5-5".into()).unwrap();
    agent.api_base = api.base.clone();
    agent
}

/// Removes the session files a test agent wrote under the test config folder.
pub(crate) fn remove_session(agent: &Agent) {
    if let Some(config_dir) = crate::config::dir() {
        let sessions = config_dir.join("sessions");
        let _ = std::fs::remove_file(sessions.join(format!("{}.jsonl", agent.session.id)));
        let _ = std::fs::remove_dir_all(sessions.join(&agent.session.id));
    }
}

pub(crate) fn session_file(agent: &Agent) -> std::path::PathBuf {
    crate::config::dir()
        .unwrap()
        .join("sessions")
        .join(format!("{}.jsonl", agent.session.id))
}
