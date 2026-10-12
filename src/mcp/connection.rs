use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::{http::HttpSession, stdio::ServerGroup};

#[derive(Default)]
pub(super) struct Pending {
    pub(super) senders: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    closed: AtomicBool,
}

impl Pending {
    pub(super) fn answer(&self, id: u64, result: Result<Value, String>) {
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|mut senders| senders.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }

    pub(super) fn is_waiting(&self, id: u64) -> bool {
        self.senders
            .lock()
            .is_ok_and(|senders| senders.contains_key(&id))
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut senders) = self.senders.lock() {
            senders.clear();
        }
    }
}

struct PendingGuard<'a> {
    pending: &'a Pending,
    id: u64,
    /// Where to tell the server the request was abandoned; `None` for
    /// `initialize`, which must never be cancelled.
    writer: Option<&'a mpsc::UnboundedSender<Value>>,
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

pub(super) struct Connection {
    pub(super) writer: mpsc::UnboundedSender<Value>,
    pub(super) pending: Arc<Pending>,
    pub(super) next_id: AtomicU64,
    pub(super) stderr: Arc<Mutex<Vec<u8>>>,
    pub(super) timeout: Duration,
    pub(super) _process_group: Option<ServerGroup>,
    pub(super) _session: Option<HttpSession>,
}

/// Answers a request from the server, or hands a response to the request
/// waiting for it. Notifications are ignored.
pub(super) fn handle_message(
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

impl Connection {
    fn with_stderr(&self, message: String) -> String {
        let tail = self
            .stderr
            .lock()
            .map(|tail| String::from_utf8_lossy(&tail).trim().to_string())
            .unwrap_or_default();
        if tail.is_empty() {
            message
        } else {
            format!("{message}\nstderr:\n{tail}")
        }
    }

    fn send(&self, message: Value) -> Result<(), String> {
        self.writer
            .send(message)
            .map_err(|_| self.with_stderr("server closed the connection".into()))
    }

    pub(super) fn notify(&self, method: &str) -> Result<(), String> {
        self.send(json!({ "jsonrpc": "2.0", "method": method }))
    }

    pub(super) async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut senders) = self.pending.senders.lock() {
            senders.insert(id, sender);
        }
        let _guard = PendingGuard {
            pending: &self.pending,
            id,
            writer: (method != "initialize").then_some(&self.writer),
        };
        if self.pending.closed.load(Ordering::SeqCst) {
            return Err(self.with_stderr("server closed the connection".into()));
        }
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(self.with_stderr("server closed the connection".into())),
            Err(_) => Err(self.with_stderr(format!(
                "{method} timed out after {} seconds",
                self.timeout.as_secs()
            ))),
        }
    }
}
