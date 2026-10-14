use std::{
    fmt::Write,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{
    DEFAULT_TIMEOUT, NEEDS_SIGN_IN,
    config_file::{ServerConfig, expand_variables, uses_oauth},
    connection::{Connection, Pending, handle_message},
    headers_helper::{HeadersHelper, header_map},
    oauth,
};

mod reply;

use reply::{read_event_stream, read_json};

pub(super) const MAX_ERROR_BODY: usize = 500;
const CONNECT_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];
const SESSION_HEADER: &str = "mcp-session-id";

struct PostError {
    message: String,
    /// A network failure, or a status that may clear up on its own.
    transient: bool,
    /// The session the server no longer knows.
    expired: Option<String>,
}

impl PostError {
    fn fatal(message: String) -> Self {
        Self {
            message,
            transient: false,
            expired: None,
        }
    }
}

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

struct OAuthSession {
    server_name: String,
    saved: Mutex<Option<oauth::Saved>>,
    refreshing: tokio::sync::Mutex<()>,
    /// What to tell the user about the last renewal, until it is shown.
    renewal_notice: Mutex<Option<String>>,
}

fn get<T: Clone + Default>(value: &Mutex<T>) -> T {
    value.lock().map(|value| value.clone()).unwrap_or_default()
}

fn set<T>(value: &Mutex<T>, new: T) {
    if let Ok(mut value) = value.lock() {
        *value = new;
    }
}

impl OAuthSession {
    fn current(&self) -> Option<oauth::Saved> {
        get(&self.saved)
    }

    fn needs_sign_in(&self, reason: Option<String>) -> PostError {
        let mut message = format!("{NEEDS_SIGN_IN}: run /mcp login {}", self.server_name);
        if let Some(reason) = reason {
            let _ = write!(message, " (could not renew the sign-in: {reason})");
        }
        PostError::fatal(message)
    }
}

/// Ends the server's session when the connection is dropped.
pub(super) struct HttpSession(Arc<Http>);

impl HttpSession {
    /// Who the server acts for, when it signs in with OAuth.
    pub(super) fn user(&self) -> Option<String> {
        let oauth = self.0.oauth.as_ref()?;
        Some(match oauth.current() {
            None => "anonymous".into(),
            Some(_) => "signed in".into(),
        })
    }

    pub(super) fn take_renewal_notice(&self) -> Option<String> {
        let oauth = self.0.oauth.as_ref()?;
        oauth.renewal_notice.lock().ok()?.take()
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        let session = get(&self.0.session);
        let (Some(session), Ok(runtime)) = (session, tokio::runtime::Handle::try_current()) else {
            return;
        };
        let mut request = self
            .0
            .client
            .delete(&self.0.url)
            .headers(get(&self.0.headers))
            .header(SESSION_HEADER, session)
            .timeout(Duration::from_secs(1));
        if let Some(version) = get(&self.0.protocol_version) {
            request = request.header("mcp-protocol-version", version);
        }
        if let Some(saved) = self.0.oauth.as_ref().and_then(OAuthSession::current) {
            request = request.bearer_auth(saved.access_token);
        }
        runtime.spawn(async move {
            let _ = request.send().await;
        });
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
    /// Posts a request. Tries `initialize` again after a transient failure,
    /// and starts a new session once when the server says it has expired.
    async fn post_request(&self, message: &Value) -> Result<reqwest::Response, String> {
        let mut delays = CONNECT_RETRY_DELAYS.iter();
        let mut renewed = false;
        loop {
            drop(self.renewing.lock().await);
            let error = match self.post(message).await {
                Ok(response) => return Ok(response),
                Err(error) => error,
            };
            if let Some(expired) = error.expired.filter(|_| !renewed) {
                renewed = true;
                self.renew(&expired).await?;
                continue;
            }
            match delays.next() {
                Some(delay) if error.transient && message["method"] == "initialize" => {
                    tokio::time::sleep(*delay).await;
                }
                _ => return Err(error.message),
            }
        }
    }

    async fn renew(&self, expired: &str) -> Result<(), String> {
        let _renewing = self.renewing.lock().await;
        let current = get(&self.session);
        if current.as_deref() != Some(expired) {
            return Ok(());
        }
        let initialize = get(&self.initialize).ok_or("session expired before it started")?;
        let previous = get(&self.protocol_version);
        set(&self.session, None);
        set(&self.protocol_version, None);
        let started = self.start_session(&initialize).await;
        if started.is_err()
            && let Ok(mut session) = self.session.lock()
            && session.is_none()
        {
            *session = current;
            set(&self.protocol_version, previous);
        }
        started
    }

    async fn start_session(&self, initialize: &Value) -> Result<(), String> {
        let failed = |error: String| format!("cannot start a new session: {error}");
        let response = self
            .post(initialize)
            .await
            .map_err(|error| failed(error.message))?;
        let mut answered = false;
        self.read_body(response, "initialize", |reply| {
            if reply["id"] != initialize["id"] {
                return false;
            }
            answered = reply["result"].is_object();
            self.negotiated(reply);
            true
        })
        .await
        .map_err(failed)?;
        if !answered {
            return Err(failed("server did not accept initialize".into()));
        }
        self.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
            .map_err(|error| failed(error.message))?;
        Ok(())
    }

    fn negotiated(&self, reply: &Value) {
        if let Some(version) = reply["result"]["protocolVersion"].as_str() {
            set(&self.protocol_version, Some(version.to_string()));
        }
    }

    /// The access token to send, renewed when it has expired or after the
    /// server turned down `rejected`.
    async fn access_token(&self, rejected: Option<&str>) -> Result<Option<String>, PostError> {
        let Some(oauth) = &self.oauth else {
            return Ok(None);
        };
        let Some(current) = oauth.current() else {
            return Ok(None);
        };
        if rejected.is_none() && !current.expired() {
            return Ok(Some(current.access_token));
        }
        let _refreshing = oauth.refreshing.lock().await;
        let Some(current) = oauth.current() else {
            return Ok(None);
        };
        if Some(current.access_token.as_str()) != rejected && !current.expired() {
            return Ok(Some(current.access_token));
        }
        let renewed = oauth::refresh(&self.client, &self.url, &current, rejected)
            .await
            .map_err(|error| oauth.needs_sign_in(Some(error)))?;
        if let Some(error) = renewed.save_error {
            let notice = format!(
                "renewed the sign-in to MCP server {}, but {error}",
                oauth.server_name
            );
            set(&oauth.renewal_notice, Some(notice));
        }
        set(&oauth.saved, Some(renewed.saved.clone()));
        Ok(Some(renewed.saved.access_token))
    }

    /// Runs the `headersHelper`, if any, and sends its headers from now on.
    async fn run_headers_helper(&self) -> Result<(), PostError> {
        let Some(helper) = &self.headers_helper else {
            return Ok(());
        };
        let dynamic = helper.run(&self.url).await.map_err(PostError::fatal)?;
        let mut headers = self.static_headers.clone();
        for (name, value) in &dynamic {
            headers.insert(name, value.clone());
        }
        set(&self.headers, headers);
        Ok(())
    }

    async fn post(&self, message: &Value) -> Result<reqwest::Response, PostError> {
        let initialize = message["method"] == "initialize";
        if initialize {
            self.run_headers_helper().await?;
        }
        let mut rejected = None;
        let mut helper_ran_again = initialize;
        loop {
            let token = self.access_token(rejected.as_deref()).await?;
            let (response, session) = self.post_once(message, token.as_deref()).await?;
            let Some(oauth) = &self.oauth else {
                if response.status().as_u16() == 401
                    && self.headers_helper.is_some()
                    && !helper_ran_again
                {
                    helper_ran_again = true;
                    self.run_headers_helper().await?;
                    continue;
                }
                return self.check(message, response, session).await;
            };
            let status = response.status().as_u16();
            let challenge = response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let unauthorised =
                status == 401 || (status == 403 && challenge.contains("insufficient_scope"));
            if !unauthorised {
                return self.check(message, response, session).await;
            }
            if rejected.is_none() && token.is_some() {
                rejected = token;
                continue;
            }
            return Err(oauth.needs_sign_in(None));
        }
    }

    async fn post_once(
        &self,
        message: &Value,
        token: Option<&str>,
    ) -> Result<(reqwest::Response, Option<String>), PostError> {
        // Each post, with the reply read from it, takes at most the server's
        // timeout, so a notification the server never answers does not hold
        // up the messages queued after it. Requests are bounded as a whole
        // by the same timeout, started before this.
        let mut request = self
            .client
            .post(&self.url)
            .headers(get(&self.headers))
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(message)
            .timeout(self.timeout);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let session = get(&self.session);
        if let Some(session) = &session {
            request = request.header(SESSION_HEADER, session);
        }
        if let Some(version) = get(&self.protocol_version) {
            request = request.header("mcp-protocol-version", version);
        }
        let response = request.send().await.map_err(|error| PostError {
            message: format!(
                "cannot reach {}: {:#}",
                self.url,
                anyhow::Error::from(error)
            ),
            transient: true,
            expired: None,
        })?;
        Ok((response, session))
    }

    /// Keeps the session the server gave, and turns error statuses into
    /// errors. `session` is the one the request was sent with.
    async fn check(
        &self,
        message: &Value,
        response: reqwest::Response,
        session: Option<String>,
    ) -> Result<reqwest::Response, PostError> {
        let method = message["method"].as_str().unwrap_or("a reply");
        if let Some(session) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|session| session.to_str().ok())
        {
            set(&self.session, Some(session.to_string()));
        }
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let body: String = body.trim().chars().take(MAX_ERROR_BODY).collect();
        let status = status.as_u16();
        let message = if body.is_empty() {
            format!("server answered {method} with status {status}")
        } else {
            format!("server answered {method} with status {status}: {body}")
        };
        Err(PostError {
            message,
            transient: status == 408 || status == 429 || (status >= 500 && status != 501),
            expired: session.filter(|_| status == 404),
        })
    }

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
