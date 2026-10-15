use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

#[derive(Default)]
pub(in crate::mcp) struct Pending {
    pub(in crate::mcp) senders: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    pub(super) closed: AtomicBool,
}

impl Pending {
    pub(in crate::mcp) fn answer(&self, id: u64, result: Result<Value, String>) {
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|mut senders| senders.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }

    pub(in crate::mcp) fn is_waiting(&self, id: u64) -> bool {
        self.senders
            .lock()
            .is_ok_and(|senders| senders.contains_key(&id))
    }

    pub(in crate::mcp) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut senders) = self.senders.lock() {
            senders.clear();
        }
    }
}

pub(super) struct PendingGuard<'a> {
    pub(super) pending: &'a Pending,
    pub(super) id: u64,
    /// Where to tell the server the request was abandoned; `None` for
    /// `initialize`, which must never be cancelled.
    pub(super) writer: Option<&'a mpsc::UnboundedSender<Value>>,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        let abandoned = self
            .pending
            .senders
            .lock()
            .is_ok_and(|mut senders| senders.remove(&self.id).is_some());
        if abandoned && let Some(writer) = self.writer {
            let _ = writer.send(json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": self.id, "reason": "the client stopped waiting" },
            }));
        }
    }
}

/// Answers a request from the server, or hands a response to the request
/// waiting for it. Notifications are ignored.
pub(in crate::mcp) fn handle_message(
    message: &Value,
    writer: &mpsc::UnboundedSender<Value>,
    pending: &Pending,
) {
    let id = &message["id"];
    if let Some(method) = message["method"].as_str() {
        if id.is_null() {
            return;
        }
        let reply = if method == "ping" {
            json!({ "jsonrpc": "2.0", "id": id, "result": {} })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") },
            })
        };
        let _ = writer.send(reply);
        return;
    }
    let Some(id) = id.as_u64() else {
        return;
    };
    let result = match message.get("error") {
        Some(error) => Err(format!(
            "{} ({})",
            error["message"].as_str().unwrap_or("unknown error"),
            error["code"]
        )),
        None => Ok(message["result"].clone()),
    };
    pending.answer(id, result);
}
