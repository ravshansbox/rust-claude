use super::super::Reply;
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, net::TcpStream};

pub(super) async fn respond(mut stream: TcpStream, reply: Option<Reply>) {
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

pub(super) async fn bad_request(stream: TcpStream, message: &str) {
    error(stream, "400 Bad Request", "invalid_request_error", message).await;
}

pub(super) async fn error(stream: TcpStream, status: &str, kind: &str, message: &str) {
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
