use crate::skills::escape_xml;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub(super) fn page(message: &str) -> String {
    let message = escape_xml(message);
    format!("<!doctype html><title>rust-coding-agent</title><p>{message}</p>")
}

pub(super) async fn reply_page(stream: &mut TcpStream, status: &str, page: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Reads the request line of a browser request, such as
/// `GET /callback?code=… HTTP/1.1`, and returns its target.
pub(super) async fn read_target(stream: &mut TcpStream) -> Option<String> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    while !data.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || data.len() > 64 * 1024 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&data);
    head.lines().next()?.split(' ').nth(1).map(str::to_string)
}
