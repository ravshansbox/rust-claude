use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;

use super::{
    Http,
    reply::{read_event_stream, read_json},
    set,
};
use crate::mcp::connection::{Pending, handle_message};

/// Posts messages in order, without waiting for the answers to requests, so
/// a notification never overtakes the request queued before it.
pub(super) async fn post_messages(
    http: Arc<Http>,
    mut messages: mpsc::UnboundedReceiver<Value>,
    writer: mpsc::WeakUnboundedSender<Value>,
    pending: Arc<Pending>,
) {
    while let Some(message) = messages.recv().await {
        if message["method"].is_string() && !message["id"].is_null() {
            tokio::spawn(
                http.clone()
                    .exchange(message, writer.clone(), pending.clone()),
            );
        } else {
            let _ = http.post(&message).await;
        }
    }
}

impl Http {
    async fn exchange(
        self: Arc<Self>,
        message: Value,
        writer: mpsc::WeakUnboundedSender<Value>,
        pending: Arc<Pending>,
    ) {
        let Some(id) = message["id"].as_u64() else {
            return;
        };
        let method = message["method"].as_str().unwrap_or_default();
        if method == "initialize" {
            set(&self.initialize, Some(message.clone()));
        }
        let handle = |reply: &Value| {
            if method == "initialize" && reply["id"] == id {
                self.negotiated(reply);
            }
            if let Some(writer) = writer.upgrade() {
                handle_message(reply, &writer, &pending);
            }
            !pending.is_waiting(id)
        };
        let replies = async {
            let response = self.post_request(&message).await?;
            self.read_body(response, method, handle).await
        };
        let error = match tokio::time::timeout(self.timeout, replies).await {
            Ok(Ok(())) => format!("server ended the reply to {method} without answering it"),
            Ok(Err(error)) => error,
            Err(_) => return,
        };
        pending.answer(id, Err(error));
    }

    /// Reads the messages in the server's reply to a request and hands each
    /// to `handle`, until `handle` says it has what it waited for.
    pub(super) async fn read_body(
        &self,
        response: reqwest::Response,
        method: &str,
        handle: impl FnMut(&Value) -> bool,
    ) -> Result<(), String> {
        if matches!(response.status().as_u16(), 202 | 204) {
            return Err(format!("server accepted {method} without a reply"));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        match content_type.as_str() {
            "application/json" => read_json(response, method, handle).await,
            "text/event-stream" => read_event_stream(response, method, handle).await,
            other => Err(format!(
                "server answered {method} with unsupported content type {other:?}"
            )),
        }
    }
}
