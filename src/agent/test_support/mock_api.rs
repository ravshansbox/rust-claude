use super::Reply;
use respond::{bad_request, error, respond};
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

mod respond;

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
