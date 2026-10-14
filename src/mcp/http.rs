use std::{
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use serde_json::Value;
use tokio::sync::mpsc;

use super::{
    DEFAULT_TIMEOUT,
    config_file::{ServerConfig, expand_variables, uses_oauth},
    connection::{Connection, Pending, handle_message},
    headers_helper::{HeadersHelper, header_map},
    oauth,
};

mod oauth_session;
mod post;
mod renewal;
mod reply;
mod session;

use oauth_session::OAuthSession;
use post::PostError;
use reply::{read_event_stream, read_json};
pub(super) use session::HttpSession;

pub(super) const MAX_ERROR_BODY: usize = 500;
const SESSION_HEADER: &str = "mcp-session-id";

/// A Streamable HTTP server. Each message is a POST; the server answers a
/// request with JSON or an event stream, and other messages with 202.
struct Http {
    client: reqwest::Client,
    url: String,
    /// The headers from the entry, before the `headersHelper` ones.
    static_headers: reqwest::header::HeaderMap,
    headers: Mutex<reqwest::header::HeaderMap>,
    headers_helper: Option<HeadersHelper>,
    session: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    timeout: Duration,
    /// The `initialize` request, sent again to start a new session.
    initialize: Mutex<Option<Value>>,
    renewing: tokio::sync::Mutex<()>,
    /// Set when the server signs in with OAuth.
    oauth: Option<OAuthSession>,
}

fn get<T: Clone + Default>(value: &Mutex<T>) -> T {
    value.lock().map(|value| value.clone()).unwrap_or_default()
}

fn set<T>(value: &Mutex<T>, new: T) {
    if let Ok(mut value) = value.lock() {
        *value = new;
    }
}

/// Posts messages in order, without waiting for the answers to requests, so
/// a notification never overtakes the request queued before it.
async fn post_messages(
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
    async fn read_body(
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

impl Connection {
    pub(super) fn http(name: &str, config: &ServerConfig, url: &str) -> Result<Self, String> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("url must be an http or https URL".into());
        }
        let headers = header_map(&config.headers, "", |name, value| {
            expand_variables(value)
                .map_err(|variable| format!("header {name} uses {variable}, which is not set"))
        })?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| format!("cannot set up HTTP: {error}"))?;
        let timeout = Duration::from_secs(config.timeout.unwrap_or(DEFAULT_TIMEOUT));
        let http = Arc::new(Http {
            client,
            url: url.to_string(),
            headers: Mutex::new(headers.clone()),
            static_headers: headers,
            headers_helper: config.headers_helper.clone().map(|command| HeadersHelper {
                command,
                server_name: name.to_string(),
            }),
            session: Mutex::new(None),
            protocol_version: Mutex::new(None),
            timeout,
            initialize: Mutex::new(None),
            renewing: tokio::sync::Mutex::new(()),
            oauth: uses_oauth(config).then(|| OAuthSession {
                server_name: name.to_string(),
                saved: Mutex::new(oauth::load_for(url, config.oauth.as_ref())),
                refreshing: tokio::sync::Mutex::new(()),
                renewal_notice: Mutex::new(None),
            }),
        });
        let (writer, messages) = mpsc::unbounded_channel();
        let pending = Arc::new(Pending::default());
        tokio::spawn(post_messages(
            http.clone(),
            messages,
            writer.downgrade(),
            pending.clone(),
        ));
        Ok(Self {
            writer,
            pending,
            next_id: AtomicU64::new(1),
            stderr: Arc::new(Mutex::new(Vec::new())),
            timeout,
            _process_group: None,
            session: Some(HttpSession(http)),
        })
    }
}
