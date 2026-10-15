use std::{
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use serde_json::Value;
use tokio::sync::mpsc;

use super::{
    DEFAULT_TIMEOUT,
    config_file::{ServerConfig, expand_variables, uses_oauth},
    connection::{Connection, Pending},
    headers_helper::{HeadersHelper, header_map},
    oauth,
};

mod exchange;
mod oauth_session;
mod post;
mod renewal;
mod reply;
mod session;

use exchange::post_messages;
use oauth_session::OAuthSession;
use post::PostError;
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
