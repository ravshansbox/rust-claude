use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::{http::HttpSession, stdio::ServerGroup};

mod pending;

use pending::PendingGuard;
pub(super) use pending::{Pending, handle_message};

pub(super) struct Connection {
    pub(super) writer: mpsc::UnboundedSender<Value>,
    pub(super) pending: Arc<Pending>,
    pub(super) next_id: AtomicU64,
    pub(super) stderr: Arc<Mutex<Vec<u8>>>,
    pub(super) timeout: Duration,
    pub(super) _process_group: Option<ServerGroup>,
    pub(super) session: Option<HttpSession>,
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

    fn closed_error(&self) -> String {
        self.with_stderr("server closed the connection".into())
    }

    fn send(&self, message: Value) -> Result<(), String> {
        self.writer.send(message).map_err(|_| self.closed_error())
    }

    pub(super) fn notify(&self, method: &str) -> Result<(), String> {
        self.send(json!({ "jsonrpc": "2.0", "method": method }))
    }

    pub(super) fn user(&self) -> Option<String> {
        self.session.as_ref()?.user()
    }

    /// What to tell the user about the last sign-in renewal, until it is
    /// shown.
    pub(super) fn take_renewal_notice(&self) -> Option<String> {
        self.session.as_ref()?.take_renewal_notice()
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
            return Err(self.closed_error());
        }
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(self.closed_error()),
            Err(_) => Err(self.with_stderr(format!(
                "{method} timed out after {} seconds",
                self.timeout.as_secs()
            ))),
        }
    }
}
