use super::{Agent, AgentEvent};
use crate::{auth::Credentials, config};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

pub(crate) enum Reply {
    Stall,
    /// These events; a string is sent as a raw frame.
    Events(Vec<Value>),
    /// These events, sent a few bytes at a time.
    Trickled(Vec<Value>),
    /// A 400 `invalid_request_error` with this message.
    BadRequest(String),
    /// A 529 overloaded error, to retry at once, whose body breaks off.
    CutOffOverloaded,
    /// A 401 `authentication_error`, as for a sign-in the API turned down.
    Unauthorized,
    /// These events, after which the connection closes before the body ends.
    CutOff(Vec<Value>),
}

/// A server that answers one quota request with `body`, and its base URL.
pub(crate) async fn quota_server(body: Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..read]);
        }
        assert!(request.starts_with(b"GET /api/oauth/usage "));
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    base
}

/// The first event of a reply, using one input and one output token.
pub(crate) fn message_start() -> Value {
    json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } })
}

/// The start of an empty text block at index 0.
pub(crate) fn text_block_start() -> Value {
    json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })
}

/// The closing usage of a reply that ends with `stop_reason`, using one output token.
pub(crate) fn message_delta(stop_reason: &str) -> Value {
    json!({ "type": "message_delta", "delta": { "stop_reason": stop_reason }, "usage": { "output_tokens": 1 } })
}

/// The last event of a reply.
pub(crate) fn message_stop() -> Value {
    json!({ "type": "message_stop" })
}

/// The end of the block at `index`.
pub(crate) fn block_stop(index: usize) -> Value {
    json!({ "type": "content_block_stop", "index": index })
}

pub(crate) fn text_reply(text: &str) -> Reply {
    stopped_reply(text, "end_turn")
}

/// A text reply that ends with `stop_reason`.
pub(crate) fn stopped_reply(text: &str, stop_reason: &str) -> Reply {
    Reply::Events(vec![
        message_start(),
        text_block_start(),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } }),
        block_stop(0),
        message_delta(stop_reason),
        message_stop(),
    ])
}

pub(crate) fn tool_reply(id: &str, name: &str, input: &Value) -> Reply {
    Reply::Events(vec![
        message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": id, "name": name, "input": input } }),
        block_stop(0),
        message_delta("tool_use"),
        message_stop(),
    ])
}

/// `reply`, reporting that the request filled the whole context window.
pub(crate) fn full_context(reply: Reply) -> Reply {
    let Reply::Events(mut events) = reply else {
        panic!("only event replies report usage");
    };
    events[0]["message"]["usage"]["input_tokens"] = json!(100_000_000);
    Reply::Events(events)
}

/// A local stand-in for the Messages API. It answers each request with the
/// next scripted reply and records the request headers and bodies it received.
pub(crate) struct MockApi {
    pub base: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
}

impl MockApi {
    pub(crate) async fn start(replies: Vec<Reply>) -> Self {
        Self::start_with_message_limit(replies, usize::MAX).await
    }

    /// Like `start`, but rejects a request as too long, without using up a
    /// scripted reply, when its messages take more than `limit` bytes of JSON.
    pub(crate) async fn start_with_message_limit(replies: Vec<Reply>, limit: usize) -> Self {
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
                if request.0.starts_with("get /api/oauth/usage ") {
                    tokio::spawn(error(
                        stream,
                        "404 Not Found",
                        "not_found_error",
                        "no quota",
                    ));
                    continue;
                }
                let size = request.1["messages"].to_string().len();
                recorded.lock().await.push(request);
                if size > limit {
                    let message = format!("prompt is too long: {size} tokens > {limit} maximum");
                    tokio::spawn(async move { bad_request(stream, &message).await });
                    continue;
                }
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
    let header_end = loop {
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        read_chunk(stream, &mut data).await?;
    };
    let headers = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while data.len() < header_end + length {
        read_chunk(stream, &mut data).await?;
    }
    let body = serde_json::from_slice(&data[header_end..]).unwrap_or(Value::Null);
    Some((headers, body))
}

pub(crate) async fn read_chunk(stream: &mut TcpStream, data: &mut Vec<u8>) -> Option<()> {
    let mut chunk = [0u8; 8192];
    let read = stream.read(&mut chunk).await.ok()?;
    if read == 0 {
        return None;
    }
    data.extend_from_slice(&chunk[..read]);
    Some(())
}

async fn respond(mut stream: TcpStream, reply: Option<Reply>) {
    match reply {
        Some(Reply::Events(events)) => {
            let body = event_stream(&events);
            write_response(stream, "200 OK", "text/event-stream", &body).await;
        }
        Some(Reply::Trickled(events)) => {
            let body = event_stream(&events);
            let _ = stream
                .write_all(event_stream_head(body.len()).as_bytes())
                .await;
            for piece in body.as_bytes().chunks(5) {
                let _ = stream.write_all(piece).await;
                let _ = stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }
        Some(Reply::Stall) => {
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                .await;
            std::future::pending::<()>().await;
        }
        Some(Reply::CutOffOverloaded) => {
            let _ = stream
                .write_all(b"HTTP/1.1 529 Overloaded\r\nretry-after: 0\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n{\"type\":")
                .await;
        }
        Some(Reply::CutOff(events)) => {
            let body = event_stream(&events);
            let head = event_stream_head(body.len() + 100);
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(body.as_bytes()).await;
        }
        Some(Reply::BadRequest(message)) => bad_request(stream, &message).await,
        Some(Reply::Unauthorized) => {
            let message = "invalid bearer token";
            error(stream, "401 Unauthorized", "authentication_error", message).await;
        }
        None => bad_request(stream, "no scripted reply").await,
    }
}

fn event_stream_head(content_length: usize) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {content_length}\r\nconnection: close\r\n\r\n"
    )
}

fn event_stream(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| match event {
            Value::String(frame) => frame.clone(),
            _ => format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            ),
        })
        .collect()
}

async fn bad_request(stream: TcpStream, message: &str) {
    error(stream, "400 Bad Request", "invalid_request_error", message).await;
}

async fn error(stream: TcpStream, status: &str, kind: &str, message: &str) {
    let body = json!({ "type": "error", "error": { "type": kind, "message": message } });
    write_response(stream, status, "application/json", &body.to_string()).await;
}

async fn write_response(mut stream: TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

pub(crate) fn credentials(expires: u64) -> Credentials {
    serde_json::from_value(json!({
        "access": "access",
        "refresh": "refresh",
        "expires": expires,
    }))
    .unwrap()
}

pub(crate) fn agent(api: &MockApi) -> Agent {
    agent_with_http(api, reqwest::Client::new())
}

pub(crate) fn agent_with_http(api: &MockApi, http: reqwest::Client) -> Agent {
    let mut agent = Agent::new(
        http,
        credentials(4_102_444_800_000),
        "claude-opus-5-5".into(),
    )
    .unwrap();
    agent.api_base = api.base.clone();
    agent
}

pub(crate) async fn run_prompt(
    api: &MockApi,
    prompt: &str,
    on_event: impl FnMut(AgentEvent),
) -> (Agent, anyhow::Result<()>) {
    let mut agent = agent(api);
    let result = agent.prompt(prompt, &[], on_event).await;
    remove_session(&agent);
    (agent, result)
}

pub(crate) fn notices(notices: &mut Vec<String>) -> impl FnMut(AgentEvent) + '_ {
    |event| {
        if let AgentEvent::Notice(notice) = event {
            notices.push(notice);
        }
    }
}

pub(crate) fn text(text: &mut String) -> impl FnMut(AgentEvent) + '_ {
    |event| {
        if let AgentEvent::Text(delta) = event {
            text.push_str(&delta);
        }
    }
}

pub(crate) fn tool_result(request: &Value) -> Value {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .rfind(|block| block["type"] == "tool_result")
        .unwrap()
        .clone()
}

/// Removes the session files a test agent wrote under the test config folder.
pub(crate) fn remove_session(agent: &Agent) {
    remove_session_files(&agent.session.id);
}

pub(crate) fn remove_session_files(session_id: &str) {
    if let Some(config_dir) = config::dir() {
        let sessions = config_dir.join("sessions");
        let _ = std::fs::remove_file(sessions.join(format!("{session_id}.jsonl")));
        let _ = std::fs::remove_dir_all(sessions.join(session_id));
    }
}

pub(crate) fn session_file(agent: &Agent) -> std::path::PathBuf {
    config::dir()
        .unwrap()
        .join("sessions")
        .join(format!("{}.jsonl", agent.session.id))
}
