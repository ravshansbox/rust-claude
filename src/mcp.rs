use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::ChildStdin,
    sync::{mpsc, oneshot},
};

mod oauth;

use crate::{
    skills::Scope,
    tools::{self, ProcessGroup},
};

const PROTOCOL_VERSION: &str = "2025-06-18";
const DEFAULT_TIMEOUT: u64 = 60;
const STDERR_TAIL: usize = 4_000;
const MAX_LINE: usize = 32 << 20;
const MAX_TOOL_NAME: usize = 64;

#[derive(Deserialize)]
struct ServerConfig {
    #[serde(rename = "type")]
    kind: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    cwd: Option<String>,
    url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    oauth: Option<OAuthConfig>,
    enabled: Option<bool>,
    timeout: Option<u64>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
}

/// HTTP servers sign in with OAuth unless their entry sets an
/// `Authorization` header.
fn uses_oauth(config: &ServerConfig) -> bool {
    !config
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn expand_home(text: &str, home: Option<&Path>) -> String {
    match (text.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => text.to_string(),
    }
}

/// Replaces each `${NAME}` with the environment variable `NAME`.
fn expand_variables(text: &str) -> Result<String, String> {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        let name = &after[..end];
        expanded.push_str(&std::env::var(name).map_err(|_| name.to_string())?);
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
}

fn read_config(
    path: &Path,
    scope: Scope,
    servers: &mut BTreeMap<String, (Scope, ServerConfig)>,
    diagnostics: &mut Vec<String>,
) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            diagnostics.push(format!("MCP: cannot read {}: {error}", path.display()));
            return;
        }
    };
    let config: Value = match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(error) => {
            diagnostics.push(format!("MCP: invalid {}: {error}", path.display()));
            return;
        }
    };
    let Some(entries) = config["mcpServers"].as_object() else {
        if !config["mcpServers"].is_null() {
            diagnostics.push(format!(
                "MCP: mcpServers in {} must be an object",
                path.display()
            ));
        }
        return;
    };
    for (name, entry) in entries {
        if !valid_server_name(name) {
            diagnostics.push(format!(
                "MCP: server name {name:?} may only contain letters, digits, _ and -"
            ));
            continue;
        }
        match serde_json::from_value::<ServerConfig>(entry.clone()) {
            Ok(server) if server.timeout == Some(0) => {
                servers.remove(name);
                diagnostics.push(format!("MCP server {name}: timeout must be at least 1"));
            }
            Ok(server) => {
                servers.insert(name.clone(), (scope, server));
            }
            Err(error) => {
                servers.remove(name);
                diagnostics.push(format!("MCP server {name}: invalid entry: {error}"));
            }
        }
    }
}

fn read_configs(
    config_dir: Option<&Path>,
    cwd: &Path,
) -> (BTreeMap<String, (Scope, ServerConfig)>, Vec<String>) {
    let mut configs = BTreeMap::new();
    let mut diagnostics = Vec::new();
    if let Some(config_dir) = config_dir {
        read_config(
            &config_dir.join("mcp.json"),
            Scope::Global,
            &mut configs,
            &mut diagnostics,
        );
    }
    let project_dir = cwd.join(".rust-claude");
    if !config_dir.is_some_and(|config_dir| crate::skills::same_dir(config_dir, &project_dir)) {
        read_config(
            &project_dir.join("mcp.json"),
            Scope::Project,
            &mut configs,
            &mut diagnostics,
        );
    }
    (configs, diagnostics)
}

struct Pending {
    senders: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    closed: AtomicBool,
}

impl Pending {
    fn answer(&self, id: u64, result: Result<Value, String>) {
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|mut senders| senders.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }

    fn is_waiting(&self, id: u64) -> bool {
        self.senders
            .lock()
            .is_ok_and(|senders| senders.contains_key(&id))
    }

    fn close(&self) {
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

struct Connection {
    writer: mpsc::UnboundedSender<Value>,
    pending: Arc<Pending>,
    next_id: AtomicU64,
    stderr: Arc<Mutex<Vec<u8>>>,
    timeout: Duration,
    _process_group: Option<ServerGroup>,
    _session: Option<HttpSession>,
}

/// The server's process group, shared by the connection and the task that
/// reaps the server. Whichever lets go first kills the group; the other
/// then finds it gone, so a recycled group id is never signalled.
struct ServerGroup(Arc<Mutex<ProcessGroup>>);

impl Drop for ServerGroup {
    fn drop(&mut self) {
        if let Ok(mut group) = self.0.lock() {
            drop(ProcessGroup(group.0.take()));
        }
    }
}

fn frame(message: &Value) -> String {
    let mut line = message.to_string();
    line.push('\n');
    line
}

/// Writes each queued line in full, even if the request that queued it was
/// cancelled, so the server never sees a line cut off midway.
async fn write_messages(
    mut stdin: ChildStdin,
    mut messages: mpsc::UnboundedReceiver<Value>,
    pending: Arc<Pending>,
) {
    while let Some(message) = messages.recv().await {
        let line = frame(&message);
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
    pending.close();
}

async fn read_messages(
    stdout: impl tokio::io::AsyncRead + Unpin,
    writer: mpsc::UnboundedSender<Value>,
    pending: Arc<Pending>,
) {
    let mut stdout = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut stdout)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > MAX_LINE {
            let error = format!(
                "server sent a line longer than {} MiB; closed the connection",
                MAX_LINE >> 20
            );
            if let Ok(mut senders) = pending.senders.lock() {
                for (_, sender) in senders.drain() {
                    let _ = sender.send(Err(error.clone()));
                }
            }
            break;
        }
        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        handle_message(&message, &writer, &pending);
    }
    pending.close();
}

/// Answers a request from the server, or hands a response to the request
/// waiting for it. Notifications are ignored.
fn handle_message(message: &Value, writer: &mpsc::UnboundedSender<Value>, pending: &Pending) {
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

async fn read_stderr(mut stderr: impl tokio::io::AsyncRead + Unpin, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0; 4096];
    while let Ok(count) = stderr.read(&mut chunk).await
        && count > 0
    {
        if let Ok(mut tail) = tail.lock() {
            tail.extend_from_slice(&chunk[..count]);
            let excess = tail.len().saturating_sub(STDERR_TAIL);
            tail.drain(..excess);
        }
    }
}

const MAX_ERROR_BODY: usize = 500;
const NEEDS_SIGN_IN: &str = "needs sign-in";
const CONNECT_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

struct PostError {
    message: String,
    /// A network failure, or a status that may clear up on its own.
    transient: bool,
    /// The session the server no longer knows.
    expired: Option<String>,
}

/// A Streamable HTTP server. Each message is a POST; the server answers a
/// request with JSON or an event stream, and other messages with 202.
struct Http {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
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
}

impl OAuthSession {
    fn current(&self) -> Option<oauth::Saved> {
        self.saved.lock().ok().and_then(|saved| saved.clone())
    }

    fn needs_sign_in(&self, reason: Option<String>) -> PostError {
        let mut message = format!("{NEEDS_SIGN_IN}: run /mcp login {}", self.server_name);
        if let Some(reason) = reason {
            message.push_str(&format!(" (could not renew the sign-in: {reason})"));
        }
        PostError {
            message,
            transient: false,
            expired: None,
        }
    }
}

/// Ends the server's session when the connection is dropped.
struct HttpSession(Arc<Http>);

impl Drop for HttpSession {
    fn drop(&mut self) {
        let session = self
            .0
            .session
            .lock()
            .ok()
            .and_then(|session| session.clone());
        let (Some(session), Ok(runtime)) = (session, tokio::runtime::Handle::try_current()) else {
            return;
        };
        let mut request = self
            .0
            .client
            .delete(&self.0.url)
            .headers(self.0.headers.clone())
            .header("mcp-session-id", session)
            .timeout(Duration::from_secs(1));
        if let Some(saved) = self.0.oauth.as_ref().and_then(OAuthSession::current) {
            request = request.bearer_auth(saved.access_token);
        }
        runtime.spawn(async move {
            let _ = request.send().await;
        });
    }
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        text.push_str(&format!(": {error}"));
        source = error.source();
    }
    text
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
        let current = self.session.lock().ok().and_then(|session| session.clone());
        if current.as_deref() != Some(expired) {
            return Ok(());
        }
        let initialize = self
            .initialize
            .lock()
            .ok()
            .and_then(|initialize| initialize.clone())
            .ok_or("session expired before it started")?;
        for value in [&self.session, &self.protocol_version] {
            if let Ok(mut value) = value.lock() {
                *value = None;
            }
        }
        let failed = |error: String| format!("cannot start a new session: {error}");
        let response = self
            .post(&initialize)
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
        if let Some(version) = reply["result"]["protocolVersion"].as_str()
            && let Ok(mut current) = self.protocol_version.lock()
        {
            *current = Some(version.to_string());
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
        if let Ok(mut saved) = oauth.saved.lock() {
            *saved = Some(renewed.clone());
        }
        Ok(Some(renewed.access_token))
    }

    async fn post(&self, message: &Value) -> Result<reqwest::Response, PostError> {
        let mut rejected = None;
        loop {
            let token = self.access_token(rejected.as_deref()).await?;
            let (response, session) = self.post_once(message, token.as_deref()).await?;
            let Some(oauth) = &self.oauth else {
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
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(message);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let session = self.session.lock().ok().and_then(|session| session.clone());
        if let Some(session) = &session {
            request = request.header("mcp-session-id", session);
        }
        if let Some(version) = self
            .protocol_version
            .lock()
            .ok()
            .and_then(|version| version.clone())
        {
            request = request.header("mcp-protocol-version", version);
        }
        let response = request.send().await.map_err(|error| PostError {
            message: format!("cannot reach {}: {}", self.url, error_chain(&error)),
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
            .get("mcp-session-id")
            .and_then(|session| session.to_str().ok())
            && let Ok(mut current) = self.session.lock()
        {
            *current = Some(session.to_string());
        }
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let body = body.trim();
        let body: String = body.chars().take(MAX_ERROR_BODY).collect();
        let message = if body.is_empty() {
            format!("server answered {method} with status {}", status.as_u16())
        } else {
            format!(
                "server answered {method} with status {}: {body}",
                status.as_u16()
            )
        };
        let status = status.as_u16();
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
        let method = message["method"].as_str().unwrap_or_default().to_string();
        if method == "initialize"
            && let Ok(mut initialize) = self.initialize.lock()
        {
            *initialize = Some(message.clone());
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
            self.read_body(response, &method, handle).await
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
        mut response: reqwest::Response,
        method: &str,
        mut handle: impl FnMut(&Value) -> bool,
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
        let too_long = || format!("server sent a reply longer than {} MiB", MAX_LINE >> 20);
        let read_error = |error: reqwest::Error| {
            format!("cannot read the reply to {method}: {}", error_chain(&error))
        };
        match content_type.as_str() {
            "application/json" => {
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(read_error)? {
                    body.extend_from_slice(&chunk);
                    if body.len() > MAX_LINE {
                        return Err(too_long());
                    }
                }
                let body: Value = serde_json::from_slice(&body)
                    .map_err(|error| format!("server sent invalid JSON: {error}"))?;
                match body {
                    Value::Array(replies) => replies.iter().for_each(|reply| {
                        handle(reply);
                    }),
                    reply => {
                        handle(&reply);
                    }
                }
                Ok(())
            }
            "text/event-stream" => {
                let mut buffer = Vec::new();
                let mut data = Vec::new();
                let mut is_message = true;
                while let Some(chunk) = response.chunk().await.map_err(read_error)? {
                    buffer.extend_from_slice(&chunk);
                    while let Some(end) = buffer.iter().position(|&byte| byte == b'\n') {
                        let mut line: Vec<u8> = buffer.drain(..=end).collect();
                        line.pop();
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        if line.is_empty() {
                            if is_message
                                && let Ok(reply) = serde_json::from_slice::<Value>(&data)
                                && handle(&reply)
                            {
                                return Ok(());
                            }
                            data.clear();
                            is_message = true;
                            continue;
                        }
                        let (field, value) = match line.iter().position(|&byte| byte == b':') {
                            Some(colon) => (&line[..colon], &line[colon + 1..]),
                            None => (&line[..], &[][..]),
                        };
                        let value = value.strip_prefix(b" ").unwrap_or(value);
                        match field {
                            b"data" => {
                                if !data.is_empty() {
                                    data.push(b'\n');
                                }
                                data.extend_from_slice(value);
                            }
                            b"event" => is_message = value == b"message",
                            _ => {}
                        }
                        if data.len() > MAX_LINE {
                            return Err(too_long());
                        }
                    }
                    if buffer.len() > MAX_LINE {
                        return Err(too_long());
                    }
                }
                Ok(())
            }
            other => Err(format!(
                "server answered {method} with unsupported content type {other:?}"
            )),
        }
    }
}

impl Connection {
    fn http(name: &str, config: &ServerConfig, url: &str) -> Result<Self, String> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("url must be an http or https URL".into());
        }
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &config.headers {
            let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| format!("invalid header name {name:?}"))?;
            let value = expand_variables(value)
                .map_err(|variable| format!("header {name} uses {variable}, which is not set"))?;
            let value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| format!("invalid value for header {name}"))?;
            headers.insert(header_name, value);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| format!("cannot set up HTTP: {error}"))?;
        let timeout = Duration::from_secs(config.timeout.unwrap_or(DEFAULT_TIMEOUT));
        let http = Arc::new(Http {
            client,
            url: url.to_string(),
            headers,
            session: Mutex::new(None),
            protocol_version: Mutex::new(None),
            timeout,
            initialize: Mutex::new(None),
            renewing: tokio::sync::Mutex::new(()),
            oauth: uses_oauth(config).then(|| OAuthSession {
                server_name: name.to_string(),
                saved: Mutex::new(oauth::load(url)),
                refreshing: tokio::sync::Mutex::new(()),
            }),
        });
        let (writer, messages) = mpsc::unbounded_channel();
        let pending = Arc::new(Pending {
            senders: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
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
            _session: Some(HttpSession(http)),
        })
    }

    fn spawn(config: &ServerConfig, command: &str) -> Result<Self, String> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut process = tokio::process::Command::new(expand_home(command, home.as_deref()));
        process
            .args(
                config
                    .args
                    .iter()
                    .map(|argument| expand_home(argument, home.as_deref())),
            )
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            let cwd = expand_home(cwd, home.as_deref());
            if !Path::new(&cwd).is_dir() {
                return Err(format!("cwd {cwd} is not a folder"));
            }
            process.current_dir(cwd);
        }
        tools::new_session(&mut process);
        let mut child = process
            .spawn()
            .map_err(|error| format!("failed to start {command}: {error}"))?;
        let process_group = Arc::new(Mutex::new(ProcessGroup(child.id())));
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("failed to capture server input and output".into());
        };
        let (writer, lines) = mpsc::unbounded_channel();
        let pending = Arc::new(Pending {
            senders: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(write_messages(stdin, lines, pending.clone()));
        tokio::spawn(read_messages(stdout, writer.clone(), pending.clone()));
        tokio::spawn(read_stderr(stderr, stderr_tail.clone()));
        // Reap the server if it exits on its own and stop anything it left
        // running; dropping the connection kills its process group first,
        // after which this wait returns too.
        let reaper_group = ServerGroup(process_group.clone());
        tokio::spawn(async move {
            let _ = child.wait().await;
            drop(reaper_group);
        });
        Ok(Self {
            writer,
            pending,
            next_id: AtomicU64::new(1),
            stderr: stderr_tail,
            timeout: Duration::from_secs(config.timeout.unwrap_or(DEFAULT_TIMEOUT)),
            _process_group: Some(ServerGroup(process_group)),
            _session: None,
        })
    }

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

    fn notify(&self, method: &str) -> Result<(), String> {
        self.send(json!({ "jsonrpc": "2.0", "method": method }))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
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

struct Tool {
    name: String,
    qualified_name: String,
    definition: Value,
}

struct Server {
    name: String,
    scope: Scope,
    connection: Connection,
    tools: Vec<Tool>,
}

fn qualified_name(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(MAX_TOOL_NAME)
        .collect()
}

fn tool_definition(qualified_name: &str, tool: &Value) -> Value {
    let mut schema = match &tool["inputSchema"] {
        Value::Object(schema) => schema.clone(),
        _ => serde_json::Map::new(),
    };
    for key in ["anyOf", "oneOf", "allOf"] {
        schema.remove(key);
    }
    schema.insert("type".into(), json!("object"));
    if !schema.get("properties").is_some_and(Value::is_object) {
        schema.insert("properties".into(), json!({}));
    }
    let description = tool["description"]
        .as_str()
        .or(tool["title"].as_str())
        .or(tool["name"].as_str())
        .unwrap_or_default();
    json!({
        "name": qualified_name,
        "description": description,
        "input_schema": schema,
    })
}

async fn list_tools(connection: &Connection) -> Result<Vec<Value>, String> {
    let mut tools = Vec::new();
    let mut cursors = std::collections::HashSet::new();
    let mut params = json!({});
    loop {
        let result = connection.request("tools/list", params).await?;
        tools.extend(result["tools"].as_array().into_iter().flatten().cloned());
        let Some(cursor) = result["nextCursor"].as_str() else {
            return Ok(tools);
        };
        if !cursors.insert(cursor.to_string()) {
            return Err(format!("tools/list repeated the cursor {cursor:?}"));
        }
        params = json!({ "cursor": cursor });
    }
}

async fn connect(name: String, scope: Scope, config: ServerConfig) -> Result<Server, String> {
    let connection = match (config.kind.as_deref(), &config.command, &config.url) {
        (Some("sse"), _, _) => {
            return Err(
                "the SSE transport is not supported; use the server's Streamable HTTP URL".into(),
            );
        }
        (Some("http" | "streamable-http"), _, Some(url)) | (None, None, Some(url)) => {
            Connection::http(&name, &config, url)?
        }
        (Some("http" | "streamable-http"), _, None) => return Err("url is missing".into()),
        (None | Some("stdio"), Some(command), _) => Connection::spawn(&config, command)?,
        (None | Some("stdio"), None, _) => return Err("command is missing".into()),
        (Some(kind), _, _) => return Err(format!("unknown type {kind:?}")),
    };
    let initialize = connection
        .request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "rust-claude", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .await?;
    connection.notify("notifications/initialized")?;
    if initialize["capabilities"]["tools"].is_null() {
        return Ok(Server {
            name,
            scope,
            connection,
            tools: Vec::new(),
        });
    }
    let tools = list_tools(&connection)
        .await?
        .iter()
        .filter_map(|tool| {
            let tool_name = tool["name"].as_str()?;
            let qualified_name = qualified_name(&name, tool_name);
            Some(Tool {
                name: tool_name.to_string(),
                definition: tool_definition(&qualified_name, tool),
                qualified_name,
            })
        })
        .collect();
    Ok(Server {
        name,
        scope,
        connection,
        tools,
    })
}

fn result_text(result: &Value) -> String {
    let mut parts = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        let mime_type = block["mimeType"].as_str().unwrap_or("unknown type");
        parts.push(match block["type"].as_str() {
            Some("text") => block["text"].as_str().unwrap_or_default().to_string(),
            Some("image") => format!("[image: {mime_type}]"),
            Some("audio") => format!("[audio: {mime_type}]"),
            Some("resource_link") => format!(
                "[resource link: {}]",
                block["uri"].as_str().unwrap_or("unknown")
            ),
            Some("resource") => match block["resource"]["text"].as_str() {
                Some(text) => text.to_string(),
                None => format!(
                    "[resource: {}]",
                    block["resource"]["uri"].as_str().unwrap_or("unknown")
                ),
            },
            _ => block.to_string(),
        });
    }
    if parts.is_empty() && !result["structuredContent"].is_null() {
        return result["structuredContent"].to_string();
    }
    parts.join("\n")
}

/// The URL of an HTTP server entry, or `None` for a stdio one.
fn http_url(config: &ServerConfig) -> Option<&str> {
    match (config.kind.as_deref(), &config.command) {
        (Some("http" | "streamable-http"), _) | (None, None) => config.url.as_deref(),
        _ => None,
    }
}

/// A sign-in waiting for the user to finish it in the browser.
pub struct SignIn {
    pub authorize_url: String,
    pending: oauth::Pending,
}

impl SignIn {
    /// Waits for the browser to come back, then saves the tokens.
    pub async fn finish(self) -> Result<(), String> {
        self.pending.finish().await
    }
}

async fn begin_sign_in_with(name: &str, config: &ServerConfig) -> Result<SignIn, String> {
    let Some(url) = http_url(config) else {
        return Err(format!("MCP server {name} is not an HTTP server"));
    };
    if !uses_oauth(config) {
        return Err(format!(
            "MCP server {name} sends its own Authorization header"
        ));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("cannot set up HTTP: {error}"))?;
    let pending = oauth::begin(&client, url, &config.oauth.clone().unwrap_or_default()).await?;
    Ok(SignIn {
        authorize_url: pending.authorize_url.clone(),
        pending,
    })
}

fn sign_out_with(name: &str, config: &ServerConfig) -> Result<String, String> {
    let Some(url) = http_url(config) else {
        return Err(format!("MCP server {name} is not an HTTP server"));
    };
    Ok(if oauth::remove(url)? {
        format!("signed out of MCP server {name}")
    } else {
        format!("MCP server {name} was not signed in")
    })
}

/// Forgets the saved sign-in for the configured server `name`.
pub fn sign_out(name: &str) -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (configs, _) = read_configs(crate::config::dir().as_deref(), &cwd);
    let Some((_, config)) = configs.get(name) else {
        return Err(format!("no MCP server named {name}"));
    };
    sign_out_with(name, config)
}

/// Starts signing in to the configured server `name`.
pub async fn begin_sign_in(name: &str) -> Result<SignIn, String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (configs, _) = read_configs(crate::config::dir().as_deref(), &cwd);
    let Some((_, config)) = configs.get(name) else {
        return Err(format!("no MCP server named {name}"));
    };
    begin_sign_in_with(name, config).await
}

#[derive(Default)]
pub struct Mcp {
    servers: Vec<Server>,
    pub diagnostics: Vec<String>,
}

pub struct Started {
    name: String,
    result: Result<Server, String>,
}

async fn start(name: String, scope: Scope, config: ServerConfig) -> Started {
    let result = connect(name.clone(), scope, config).await;
    Started { name, result }
}

#[cfg(test)]
pub fn failed_start(name: &str, error: &str) -> Started {
    Started {
        name: name.into(),
        result: Err(error.into()),
    }
}

pub type Starting = std::pin::Pin<Box<dyn Future<Output = Started> + Send>>;

pub struct StartingServer {
    pub name: String,
    pub label: String,
    pub started: Starting,
}

pub struct Added {
    pub name: String,
    pub status: String,
    pub diagnostics: Vec<String>,
}

pub struct Startup {
    pub diagnostics: Vec<String>,
    pub servers: Vec<StartingServer>,
    /// HTTP servers that sign in with OAuth.
    pub sign_in_servers: Vec<String>,
}

fn starting(name: String, scope: Scope, config: ServerConfig) -> StartingServer {
    StartingServer {
        label: format!("{scope} MCP server: {name}"),
        started: Box::pin(start(name.clone(), scope, config)),
        name,
    }
}

/// Starts the configured server `name` again, as after signing in.
pub fn restart(name: &str) -> Option<StartingServer> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (mut configs, _) = read_configs(crate::config::dir().as_deref(), &cwd);
    let (scope, config) = configs.remove(name)?;
    Some(starting(name.to_string(), scope, config))
}

pub fn startup() -> Startup {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (configs, diagnostics) = read_configs(crate::config::dir().as_deref(), &cwd);
    let enabled: Vec<_> = configs
        .into_iter()
        .filter(|(_, (_, config))| config.enabled != Some(false))
        .collect();
    let sign_in_servers = enabled
        .iter()
        .filter(|(_, (_, config))| http_url(config).is_some() && uses_oauth(config))
        .map(|(name, _)| name.clone())
        .collect();
    let servers = enabled
        .into_iter()
        .map(|(name, (scope, config))| starting(name, scope, config))
        .collect();
    Startup {
        diagnostics,
        servers,
        sign_in_servers,
    }
}

impl Mcp {
    pub async fn load() -> Self {
        let startup = startup();
        let mut mcp = Self {
            servers: Vec::new(),
            diagnostics: startup.diagnostics,
        };
        let servers = startup.servers.into_iter().map(|pending| pending.started);
        for started in futures::future::join_all(servers).await {
            let (diagnostics, status) = mcp.add_server(started);
            mcp.diagnostics.extend(diagnostics);
            if let Err(failed) = status {
                mcp.diagnostics.push(failed);
            }
        }
        mcp
    }

    pub fn add(&mut self, started: Started) -> Added {
        let name = started.name.clone();
        let (diagnostics, status) = self.add_server(started);
        Added {
            name,
            status: status.unwrap_or_else(|failed| failed),
            diagnostics,
        }
    }

    /// Adds a server that started again in place of the old one.
    pub fn replace(&mut self, started: Started) -> Added {
        self.servers.retain(|server| server.name != started.name);
        self.add(started)
    }

    fn add_server(&mut self, started: Started) -> (Vec<String>, Result<String, String>) {
        let Started { name, result } = started;
        let mut server = match result {
            Ok(server) => server,
            Err(error) if error.starts_with(NEEDS_SIGN_IN) => {
                return (Vec::new(), Err(format!("MCP server {name} {error}")));
            }
            Err(error) => {
                return (
                    Vec::new(),
                    Err(format!("MCP server {name} failed: {error}")),
                );
            }
        };
        let mut diagnostics = Vec::new();
        let mut seen: HashMap<String, String> = self
            .servers
            .iter()
            .flat_map(|server| {
                server.tools.iter().map(|tool| {
                    (
                        tool.qualified_name.clone(),
                        format!("{}/{}", server.name, tool.name),
                    )
                })
            })
            .collect();
        server.tools.retain(|tool| {
            if let Some(other) = seen.get(&tool.qualified_name) {
                diagnostics.push(format!(
                    "MCP server {name}: skipped tool {} because its name clashes with {other}",
                    tool.name
                ));
                return false;
            }
            seen.insert(tool.qualified_name.clone(), format!("{name}/{}", tool.name));
            true
        });
        let loaded = format!(
            "loaded {} MCP server: {name} ({} tools)",
            server.scope,
            server.tools.len()
        );
        self.servers.push(server);
        (diagnostics, Ok(loaded))
    }

    pub fn definitions(&self) -> impl Iterator<Item = Value> + '_ {
        self.servers
            .iter()
            .flat_map(|server| &server.tools)
            .map(|tool| tool.definition.clone())
    }

    pub async fn call(&self, name: &str, input: &Value) -> Option<Result<String, String>> {
        let (server, tool) = self.servers.iter().find_map(|server| {
            server
                .tools
                .iter()
                .find(|tool| tool.qualified_name == name)
                .map(|tool| (server, tool))
        })?;
        let arguments = if input.is_object() {
            input.clone()
        } else {
            json!({})
        };
        let result = server
            .connection
            .request(
                "tools/call",
                json!({ "name": tool.name, "arguments": arguments }),
            )
            .await
            .map(|result| {
                let text = tools::truncate(result_text(&result));
                if result["isError"] == true {
                    Err(text)
                } else {
                    Ok(text)
                }
            });
        Some(result.and_then(|result| result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualifies_tool_names() {
        assert_eq!(qualified_name("docs", "search"), "mcp__docs__search");
        assert_eq!(
            qualified_name("docs", "get.page/v2"),
            "mcp__docs__get_page_v2"
        );
        assert_eq!(
            qualified_name("docs", &"a".repeat(100)).len(),
            MAX_TOOL_NAME
        );
    }

    #[test]
    fn makes_object_schemas() {
        let definition = tool_definition(
            "mcp__docs__search",
            &json!({ "name": "search", "inputSchema": { "type": "object" } }),
        );
        assert_eq!(
            definition,
            json!({
                "name": "mcp__docs__search",
                "description": "search",
                "input_schema": { "type": "object", "properties": {} },
            })
        );
    }

    #[test]
    fn drops_top_level_schema_combinators() {
        let definition = tool_definition(
            "mcp__docs__search",
            &json!({
                "name": "search",
                "inputSchema": {
                    "type": "object",
                    "properties": { "a": { "type": "string" }, "b": { "type": "string" } },
                    "anyOf": [{ "required": ["a"] }, { "required": ["b"] }],
                    "oneOf": [{ "required": ["a"] }],
                    "allOf": [{ "required": ["b"] }],
                },
            }),
        );
        assert_eq!(
            definition["input_schema"],
            json!({
                "type": "object",
                "properties": { "a": { "type": "string" }, "b": { "type": "string" } },
            })
        );
    }

    #[test]
    fn converts_results_to_text() {
        let result = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "image", "data": "", "mimeType": "image/png" },
                { "type": "resource", "resource": { "uri": "file:///a", "text": "body" } },
                { "type": "resource_link", "uri": "file:///b", "name": "b" },
                { "type": "resource", "resource": { "uri": "file:///c", "blob": "" } },
                { "type": "resource_link" },
            ]
        });
        assert_eq!(
            result_text(&result),
            "hello\n[image: image/png]\nbody\n[resource link: file:///b]\n[resource: file:///c]\n[resource link: unknown]"
        );
        assert_eq!(
            result_text(&json!({ "content": [], "structuredContent": { "a": 1 } })),
            r#"{"a":1}"#
        );
    }

    #[test]
    fn project_entries_replace_global_entries() {
        let directory = std::env::temp_dir().join(format!("mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let global = directory.join("global.json");
        let project = directory.join("project.json");
        std::fs::write(
            &global,
            r#"{ "mcpServers": { "docs": { "command": "a" }, "bad name": { "command": "b" } } }"#,
        )
        .unwrap();
        std::fs::write(
            &project,
            r#"{ "mcpServers": { "docs": { "command": "c" } } }"#,
        )
        .unwrap();
        let mut servers = BTreeMap::new();
        let mut diagnostics = Vec::new();
        read_config(&global, Scope::Global, &mut servers, &mut diagnostics);
        read_config(&project, Scope::Project, &mut servers, &mut diagnostics);
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers["docs"].0, Scope::Project);
        assert_eq!(servers["docs"].1.command.as_deref(), Some("c"));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn reads_oauth_settings_from_global_and_project_configs() {
        let directory = std::env::temp_dir().join(format!("mcp-oauth-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("mcp.json");
        std::fs::write(
            &path,
            r#"{ "mcpServers": { "figma": { "url": "https://mcp.figma.com/mcp", "oauth": { "clientId": "own" } } } }"#,
        )
        .unwrap();
        for scope in [Scope::Global, Scope::Project] {
            let mut servers = BTreeMap::new();
            let mut diagnostics = Vec::new();
            read_config(&path, scope, &mut servers, &mut diagnostics);
            assert!(diagnostics.is_empty());
            assert_eq!(servers["figma"].0, scope);
            assert_eq!(
                servers["figma"]
                    .1
                    .oauth
                    .as_ref()
                    .and_then(|oauth| oauth.client_id.as_deref()),
                Some("own")
            );
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn rejects_a_zero_timeout() {
        let path = std::env::temp_dir().join(format!("mcp-timeout-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{ "mcpServers": { "docs": { "command": "a", "timeout": 0 } } }"#,
        )
        .unwrap();
        let mut servers = BTreeMap::new();
        let mut diagnostics = Vec::new();
        read_config(&path, Scope::Project, &mut servers, &mut diagnostics);
        std::fs::remove_file(&path).unwrap();
        assert!(servers.is_empty());
        assert_eq!(diagnostics, ["MCP server docs: timeout must be at least 1"]);
    }

    #[test]
    fn reports_an_unreadable_config() {
        let path = std::env::temp_dir().join(format!("mcp-unreadable-{}.json", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let mut servers = BTreeMap::new();
        let mut diagnostics = Vec::new();
        read_config(&path, Scope::Project, &mut servers, &mut diagnostics);
        read_config(
            &path.join("missing.json"),
            Scope::Project,
            &mut servers,
            &mut diagnostics,
        );
        std::fs::remove_dir_all(&path).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].starts_with(&format!("MCP: cannot read {}: ", path.display())));
    }

    #[test]
    fn labels_servers_global_when_run_from_the_home_folder() {
        let home = std::env::temp_dir().join(format!("mcp-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".rust-claude")).unwrap();
        std::fs::write(
            home.join(".rust-claude/mcp.json"),
            r#"{ "mcpServers": { "docs": { "command": "a" } } }"#,
        )
        .unwrap();
        let (servers, diagnostics) = read_configs(Some(&home.join(".rust-claude")), &home);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(servers["docs"].0, Scope::Global);
        assert!(diagnostics.is_empty());
    }

    fn echo_server() -> ServerConfig {
        server_answering_calls_with(
            r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
        )
    }

    fn server_answering_calls_with(call: &str) -> ServerConfig {
        server_with("", call)
    }

    /// A bash server; `cases` are extra `case` arms tried before the defaults.
    fn server_with(cases: &str, call: &str) -> ServerConfig {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
CASES
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"t","version":"1"}}}\n' "$id" ;;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"tools/call"'*) CALL ;;
  esac
done
"#
        .replace("CASES", cases)
        .replace("CALL", call);
        ServerConfig {
            kind: None,
            command: Some("bash".into()),
            args: vec!["-c".into(), script],
            env: HashMap::new(),
            cwd: None,
            url: None,
            headers: HashMap::new(),
            oauth: None,
            enabled: None,
            timeout: Some(5),
        }
    }

    #[derive(Clone)]
    struct HttpRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Value,
        raw_body: String,
    }

    impl HttpRequest {
        fn form(&self) -> HashMap<String, String> {
            reqwest::Url::parse(&format!("http://form/?{}", self.raw_body))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect()
        }
    }

    struct HttpReply {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl HttpReply {
        fn json(body: Value) -> Self {
            Self {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: body.to_string(),
            }
        }

        fn events(events: &[Value]) -> Self {
            Self {
                status: 200,
                headers: vec![("content-type".into(), "text/event-stream".into())],
                body: events
                    .iter()
                    .map(|event| format!("event: message\ndata: {event}\n\n"))
                    .collect(),
            }
        }

        fn status(status: u16, body: &str) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: body.into(),
            }
        }

        fn stall() -> Self {
            Self::status(0, "")
        }

        fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.retain(|(existing, _)| existing != name);
            self.headers.push((name.into(), value.into()));
            self
        }
    }

    type Requests = Arc<Mutex<Vec<HttpRequest>>>;

    /// A local HTTP server that answers each request with `handler` and
    /// records the requests it received.
    async fn http_server(
        handler: impl Fn(&HttpRequest) -> HttpReply + Send + Sync + 'static,
    ) -> (String, Requests) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handler = handler.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let Some(request) = read_http_request(&mut stream).await else {
                        return;
                    };
                    recorded.lock().unwrap().push(request.clone());
                    let reply = handler(&request);
                    if reply.status == 0 {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        return;
                    }
                    let mut response = format!(
                        "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (name, value) in reply.headers {
                        response.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response.push_str("\r\n");
                    response.push_str(&reply.body);
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        (url, requests)
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Option<HttpRequest> {
        let mut data = Vec::new();
        let mut chunk = [0u8; 8192];
        let header_end = loop {
            if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                return None;
            }
            data.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8_lossy(&data[..header_end]).to_string();
        let mut lines = head.lines();
        let mut request_line = lines.next()?.split(' ');
        let method = request_line.next()?.to_string();
        let path = request_line.next()?.to_string();
        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
            .collect();
        let length: usize = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        while data.len() < header_end + length {
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                return None;
            }
            data.extend_from_slice(&chunk[..read]);
        }
        let body = serde_json::from_slice(&data[header_end..]).unwrap_or(Value::Null);
        Some(HttpRequest {
            method,
            path,
            headers,
            body,
            raw_body: String::from_utf8_lossy(&data[header_end..]).into_owned(),
        })
    }

    /// Answers like a Streamable HTTP server that starts session `s1`.
    fn http_mcp(request: &HttpRequest) -> HttpReply {
        let id = &request.body["id"];
        match request.body["method"].as_str() {
            Some("initialize") => HttpReply::json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "t", "version": "1" },
                },
            }))
            .with_header("mcp-session-id", "s1"),
            Some("tools/list") => HttpReply::json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] },
            })),
            Some("tools/call") => HttpReply::json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{ "type": "text", "text": "pong" }] },
            })),
            _ => HttpReply::status(202, ""),
        }
    }

    fn http_config(url: &str) -> ServerConfig {
        ServerConfig {
            kind: None,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            url: Some(url.into()),
            headers: HashMap::new(),
            oauth: None,
            enabled: None,
            timeout: Some(5),
        }
    }

    fn requests_for(requests: &Requests, method: &str) -> Vec<HttpRequest> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.body["method"] == method)
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn calls_http_server_tools() {
        let (url, requests) = http_server(http_mcp).await;
        let mut config = http_config(&url);
        config.kind = Some("http".into());
        config
            .headers
            .insert("Authorization".into(), "Bearer secret".into());
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, config).await)
                .status,
            "loaded global MCP server: web (1 tools)"
        );
        assert_eq!(
            mcp.call("mcp__web__echo", &json!({})).await,
            Some(Ok("pong".into()))
        );
        let initialize = &requests_for(&requests, "initialize")[0];
        assert_eq!(initialize.method, "POST");
        assert_eq!(initialize.headers["authorization"], "Bearer secret");
        assert_eq!(
            initialize.headers["accept"],
            "application/json, text/event-stream"
        );
        assert!(!initialize.headers.contains_key("mcp-session-id"));
        for method in ["notifications/initialized", "tools/list", "tools/call"] {
            let request = &requests_for(&requests, method)[0];
            assert_eq!(request.headers["mcp-session-id"], "s1", "{method}");
            assert_eq!(
                request.headers["mcp-protocol-version"], "2025-03-26",
                "{method}"
            );
            assert_eq!(request.headers["authorization"], "Bearer secret");
        }
    }

    #[tokio::test]
    async fn reads_http_replies_sent_as_events() {
        let (url, requests) = http_server(|request| {
            let id = &request.body["id"];
            match request.body["method"].as_str() {
                Some("tools/call") => HttpReply::events(&[
                    json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} }),
                    json!({ "jsonrpc": "2.0", "id": "p1", "method": "ping" }),
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "content": [{ "type": "text", "text": "streamed" }] },
                    }),
                ]),
                _ => http_mcp(request),
            }
        })
        .await;
        let mut mcp = Mcp::default();
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
        assert_eq!(
            mcp.call("mcp__web__echo", &json!({})).await,
            Some(Ok("streamed".into()))
        );
        let mut replies = Vec::new();
        for _ in 0..50 {
            replies = requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.body["id"] == "p1")
                .map(|request| request.body.clone())
                .collect();
            if !replies.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            replies,
            [json!({ "jsonrpc": "2.0", "id": "p1", "result": {} })]
        );
    }

    #[tokio::test]
    async fn reports_http_errors() {
        let (url, _) = http_server(|request| match request.body["method"].as_str() {
            Some("tools/call") => HttpReply::status(202, ""),
            Some("tools/list") => HttpReply::status(500, "database is down"),
            _ => http_mcp(request),
        })
        .await;
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "MCP server web failed: server answered tools/list with status 500: database is down"
        );
        let (url, _) = http_server(|request| match request.body["method"].as_str() {
            Some("tools/call") => HttpReply::status(202, ""),
            _ => http_mcp(request),
        })
        .await;
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
        assert_eq!(
            mcp.call("mcp__web__echo", &json!({})).await,
            Some(Err("server accepted tools/call without a reply".into()))
        );
    }

    #[tokio::test]
    async fn times_out_when_an_http_server_does_not_answer() {
        let (url, _) = http_server(|request| match request.body["method"].as_str() {
            Some("tools/call") => HttpReply::stall(),
            _ => http_mcp(request),
        })
        .await;
        let mut config = http_config(&url);
        config.timeout = Some(1);
        let mut mcp = Mcp::default();
        mcp.add(start("web".into(), Scope::Global, config).await);
        assert_eq!(
            mcp.call("mcp__web__echo", &json!({})).await,
            Some(Err("tools/call timed out after 1 seconds".into()))
        );
    }

    #[tokio::test]
    async fn expands_environment_variables_in_http_headers() {
        let (url, requests) = http_server(http_mcp).await;
        let mut config = http_config(&url);
        config
            .headers
            .insert("X-Home".into(), "home=${HOME}, cost=$5".into());
        let mut mcp = Mcp::default();
        mcp.add(start("web".into(), Scope::Global, config).await);
        let initialize = &requests_for(&requests, "initialize")[0];
        assert_eq!(
            initialize.headers["x-home"],
            format!("home={}, cost=$5", std::env::var("HOME").unwrap())
        );
        let mut config = http_config(&url);
        config.headers.insert(
            "Authorization".into(),
            "Bearer ${RUST_CLAUDE_UNSET_TOKEN}".into(),
        );
        assert_eq!(
            mcp.add(start("unset".into(), Scope::Global, config).await)
                .status,
            "MCP server unset failed: header Authorization uses RUST_CLAUDE_UNSET_TOKEN, which is not set"
        );
    }

    #[tokio::test]
    async fn retries_connecting_after_a_transient_http_error() {
        let attempts = Arc::new(AtomicU64::new(0));
        let counted = attempts.clone();
        let (url, requests) = http_server(move |request| {
            if request.body["method"] == "initialize" && counted.fetch_add(1, Ordering::SeqCst) < 2
            {
                return HttpReply::status(503, "starting up");
            }
            http_mcp(request)
        })
        .await;
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "loaded global MCP server: web (1 tools)"
        );
        assert_eq!(requests_for(&requests, "initialize").len(), 3);
        let (url, requests) = http_server(|request| match request.body["method"].as_str() {
            Some("initialize") => HttpReply::status(400, "bad request"),
            _ => http_mcp(request),
        })
        .await;
        assert_eq!(
            mcp.add(start("locked".into(), Scope::Global, http_config(&url)).await)
                .status,
            "MCP server locked failed: server answered initialize with status 400: bad request"
        );
        assert_eq!(requests_for(&requests, "initialize").len(), 1);
    }

    #[tokio::test]
    async fn starts_a_new_http_session_when_the_old_one_expires() {
        let sessions = Arc::new(AtomicU64::new(0));
        let issued = sessions.clone();
        let (url, requests) = http_server(move |request| {
            let session = request.headers.get("mcp-session-id").map(String::as_str);
            match request.body["method"].as_str() {
                Some("initialize") => {
                    let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
                    http_mcp(request).with_header("mcp-session-id", &format!("s{number}"))
                }
                Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
                _ => http_mcp(request),
            }
        })
        .await;
        let mut mcp = Mcp::default();
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
        assert_eq!(
            mcp.call("mcp__web__echo", &json!({})).await,
            Some(Ok("pong".into()))
        );
        let sessions_used = |method: &str| -> Vec<String> {
            requests_for(&requests, method)
                .iter()
                .map(|request| {
                    request
                        .headers
                        .get("mcp-session-id")
                        .cloned()
                        .unwrap_or_default()
                })
                .collect()
        };
        assert_eq!(sessions_used("initialize"), ["", ""]);
        assert_eq!(sessions_used("notifications/initialized"), ["s1", "s2"]);
        assert_eq!(sessions_used("tools/call"), ["s1", "s2"]);
    }

    fn now_millis() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    }

    /// Answers like `http_mcp`, but only with the access token `fresh`, and
    /// renews the refresh token `r1` at `/token`.
    fn signed_in_mcp(request: &HttpRequest) -> HttpReply {
        if request.path == "/token" {
            let form = request.form();
            if form.get("grant_type").map(String::as_str) == Some("refresh_token")
                && form.get("refresh_token").map(String::as_str) == Some("r1")
                && form.get("client_id").map(String::as_str) == Some("client")
            {
                return HttpReply::json(json!({
                    "access_token": "fresh",
                    "refresh_token": "r2",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                }));
            }
            return HttpReply::status(400, r#"{"error":"invalid_grant"}"#);
        }
        if request.headers.get("authorization").map(String::as_str) != Some("Bearer fresh") {
            return HttpReply::status(401, "")
                .with_header("www-authenticate", r#"Bearer error="invalid_token""#);
        }
        http_mcp(request)
    }

    fn save_tokens(url: &str, access_token: &str, expires_at: u128) {
        let token_endpoint = url.replace("/mcp", "/token");
        oauth::save(
            url,
            &oauth::Saved {
                client_id: "client".into(),
                client_secret: None,
                token_endpoint,
                resource: Some(url.into()),
                access_token: access_token.into(),
                refresh_token: Some("r1".into()),
                expires_at: Some(expires_at),
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn renews_oauth_tokens_that_an_http_server_turns_down() {
        let (url, requests) = http_server(signed_in_mcp).await;
        save_tokens(&url, "stale", now_millis() + 3_600_000);
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "loaded global MCP server: web (1 tools)"
        );
        assert_eq!(
            mcp.call("mcp__web__echo", &json!({})).await,
            Some(Ok("pong".into()))
        );
        let saved = oauth::load(&url).unwrap();
        assert_eq!(saved.access_token, "fresh");
        assert_eq!(saved.refresh_token.as_deref(), Some("r2"));
        let tokens_sent: Vec<String> = requests_for(&requests, "initialize")
            .iter()
            .map(|request| request.headers["authorization"].clone())
            .collect();
        assert_eq!(tokens_sent, ["Bearer stale", "Bearer fresh"]);
    }

    #[tokio::test]
    async fn renews_expired_oauth_tokens_before_sending_them() {
        let (url, requests) = http_server(signed_in_mcp).await;
        save_tokens(&url, "stale", now_millis() - 1);
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "loaded global MCP server: web (1 tools)"
        );
        let tokens_sent: Vec<String> = requests_for(&requests, "initialize")
            .iter()
            .map(|request| request.headers["authorization"].clone())
            .collect();
        assert_eq!(tokens_sent, ["Bearer fresh"]);
    }

    #[tokio::test]
    async fn asks_to_sign_in_when_an_http_server_needs_it() {
        let (url, _) = http_server(signed_in_mcp).await;
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "MCP server web needs sign-in: run /mcp login web"
        );
        let (url, _) = http_server(signed_in_mcp).await;
        let mut config = http_config(&url);
        config
            .headers
            .insert("authorization".into(), "Bearer wrong".into());
        assert_eq!(
            mcp.add(start("keyed".into(), Scope::Global, config).await)
                .status,
            "MCP server keyed failed: server answered initialize with status 401"
        );
    }

    /// Answers like a server that needs OAuth, with its sign-in server at
    /// `/auth` on the same host, and records the forms sent to `/register`
    /// and `/token`.
    fn oauth_mcp(base: &str, request: &HttpRequest) -> HttpReply {
        let url = format!("{base}/mcp");
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/.well-known/oauth-protected-resource/mcp") => HttpReply::json(json!({
                "resource": url,
                "authorization_servers": [format!("{base}/auth")],
                "scopes_supported": ["unused"],
            })),
            ("GET", "/.well-known/oauth-authorization-server/auth") => HttpReply::json(json!({
                "issuer": format!("{base}/auth"),
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            })),
            ("POST", "/register") if request.body["client_name"] == "Claude Code" => {
                HttpReply::json(json!({ "client_id": "registered", "client_secret": "shh" }))
            }
            ("POST", "/register") => HttpReply::status(403, "Forbidden"),
            ("POST", "/token") if request.form().get("code").map(String::as_str) == Some("the-code") => {
                HttpReply::json(json!({
                    "access_token": "issued",
                    "refresh_token": "r",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                }))
            }
            ("POST", "/token") => HttpReply::status(400, r#"{"error":"invalid_grant"}"#),
            _ if request.headers.get("authorization").map(String::as_str)
                == Some("Bearer issued") =>
            {
                http_mcp(request)
            }
            _ => HttpReply::status(401, "").with_header(
                "www-authenticate",
                &format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp", scope="mcp:connect""#
                ),
            ),
        }
    }

    async fn oauth_server() -> (String, Requests) {
        let base = Arc::new(Mutex::new(String::new()));
        let shared = base.clone();
        let (url, requests) =
            http_server(move |request| oauth_mcp(&shared.lock().unwrap(), request)).await;
        *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
        (url, requests)
    }

    fn query(url: &str) -> HashMap<String, String> {
        reqwest::Url::parse(url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect()
    }

    /// Opens the redirect the sign-in server would send the browser to.
    async fn visit_callback(authorize_url: &str, code: &str, state: Option<&str>) -> String {
        let parameters = query(authorize_url);
        let mut callback = reqwest::Url::parse(&parameters["redirect_uri"]).unwrap();
        callback
            .query_pairs_mut()
            .append_pair("code", code)
            .append_pair("state", state.unwrap_or(&parameters["state"]));
        reqwest::get(callback).await.unwrap().text().await.unwrap()
    }

    #[tokio::test]
    async fn signs_in_to_an_http_server_with_oauth() {
        let (url, requests) = oauth_server().await;
        let base = url.trim_end_matches("/mcp");
        let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
        let parameters = query(&sign_in.authorize_url);
        assert!(
            sign_in
                .authorize_url
                .starts_with(&format!("{base}/authorize?"))
        );
        assert_eq!(parameters["response_type"], "code");
        assert_eq!(parameters["client_id"], "registered");
        assert_eq!(parameters["code_challenge_method"], "S256");
        assert_eq!(parameters["scope"], "mcp:connect");
        assert_eq!(parameters["resource"], url);
        assert!(parameters["redirect_uri"].starts_with("http://127.0.0.1:"));
        assert!(parameters["redirect_uri"].ends_with("/callback"));
        let authorize_url = sign_in.authorize_url.clone();
        let finished = tokio::spawn(sign_in.finish());
        let page = visit_callback(&authorize_url, "the-code", None).await;
        assert!(page.contains("Signed in"), "{page}");
        assert_eq!(finished.await.unwrap(), Ok(()));
        let registration = &requests_for_path(&requests, "/register")[0];
        assert_eq!(
            registration.body["redirect_uris"],
            json!([parameters["redirect_uri"]])
        );
        let token_form = requests_for_path(&requests, "/token")[0].form();
        assert_eq!(token_form["grant_type"], "authorization_code");
        assert_eq!(token_form["client_id"], "registered");
        assert_eq!(token_form["client_secret"], "shh");
        assert_eq!(token_form["redirect_uri"], parameters["redirect_uri"]);
        assert_eq!(token_form["resource"], url);
        use base64::Engine;
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            <sha2::Sha256 as sha2::Digest>::digest(token_form["code_verifier"].as_bytes()),
        );
        assert_eq!(parameters["code_challenge"], challenge);
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "loaded global MCP server: web (1 tools)"
        );
    }

    fn requests_for_path(requests: &Requests, path: &str) -> Vec<HttpRequest> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path == path)
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn signs_in_with_a_client_id_from_the_config() {
        let (url, requests) = oauth_server().await;
        let mut config = http_config(&url);
        config.oauth = Some(OAuthConfig {
            client_id: Some("own".into()),
            ..OAuthConfig::default()
        });
        let sign_in = begin_sign_in_with("web", &config).await.unwrap();
        assert_eq!(query(&sign_in.authorize_url)["client_id"], "own");
        assert!(requests_for_path(&requests, "/register").is_empty());
    }

    #[tokio::test]
    async fn registers_as_claude_code() {
        let (url, requests) = oauth_server().await;
        begin_sign_in_with("web", &http_config(&url)).await.unwrap();
        assert_eq!(
            requests_for_path(&requests, "/register")[0].body["client_name"],
            "Claude Code"
        );
    }

    #[tokio::test]
    async fn reports_failed_sign_ins() {
        let (url, _) = oauth_server().await;
        let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
        let authorize_url = sign_in.authorize_url.clone();
        let finished = tokio::spawn(sign_in.finish());
        let page = visit_callback(&authorize_url, "the-code", Some("forged")).await;
        assert!(page.contains("Sign-in failed"), "{page}");
        assert_eq!(
            finished.await.unwrap(),
            Err("the sign-in page sent back a different state".into())
        );
        let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
        let authorize_url = sign_in.authorize_url.clone();
        let finished = tokio::spawn(sign_in.finish());
        visit_callback(&authorize_url, "wrong-code", None).await;
        assert_eq!(
            finished.await.unwrap(),
            Err("token endpoint answered with status 400: invalid_grant".into())
        );
        let mut keyed = http_config(&url);
        keyed
            .headers
            .insert("Authorization".into(), "Bearer key".into());
        assert_eq!(
            begin_sign_in_with("keyed", &keyed).await.err(),
            Some("MCP server keyed sends its own Authorization header".into())
        );
        assert_eq!(
            begin_sign_in_with("local", &echo_server()).await.err(),
            Some("MCP server local is not an HTTP server".into())
        );
    }

    #[tokio::test]
    async fn replaces_a_server_that_starts_again() {
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, echo_server()).await);
        assert_eq!(
            mcp.replace(start("test".into(), Scope::Project, echo_server()).await)
                .status,
            "loaded project MCP server: test (1 tools)"
        );
        assert_eq!(mcp.definitions().count(), 1);
        mcp.replace(failed_start("test", "gone"));
        assert_eq!(mcp.definitions().count(), 0);
    }

    #[tokio::test]
    async fn signs_out_of_an_http_server() {
        let (url, _) = http_server(signed_in_mcp).await;
        save_tokens(&url, "fresh", now_millis() + 3_600_000);
        assert_eq!(
            sign_out_with("web", &http_config(&url)),
            Ok("signed out of MCP server web".into())
        );
        assert!(oauth::load(&url).is_none());
        assert_eq!(
            sign_out_with("web", &http_config(&url)),
            Ok("MCP server web was not signed in".into())
        );
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, http_config(&url)).await)
                .status,
            "MCP server web needs sign-in: run /mcp login web"
        );
        assert_eq!(
            sign_out_with("local", &echo_server()),
            Err("MCP server local is not an HTTP server".into())
        );
    }

    #[tokio::test]
    async fn rejects_unsupported_http_servers() {
        let mut mcp = Mcp::default();
        let mut sse = http_config("http://127.0.0.1:1/sse");
        sse.kind = Some("sse".into());
        assert_eq!(
            mcp.add(start("old".into(), Scope::Global, sse).await)
                .status,
            "MCP server old failed: the SSE transport is not supported; use the server's Streamable HTTP URL"
        );
        assert_eq!(
            mcp.add(
                start(
                    "ftp".into(),
                    Scope::Global,
                    http_config("ftp://example.com")
                )
                .await
            )
            .status,
            "MCP server ftp failed: url must be an http or https URL"
        );
    }

    #[tokio::test]
    async fn ends_the_http_session_when_dropped() {
        let (url, requests) = http_server(http_mcp).await;
        let mut mcp = Mcp::default();
        mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
        drop(mcp);
        let mut deleted = Vec::new();
        for _ in 0..50 {
            deleted = requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.method == "DELETE")
                .map(|request| request.headers["mcp-session-id"].clone())
                .collect();
            if !deleted.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(deleted, ["s1"]);
    }

    #[tokio::test]
    async fn shows_server_errors_when_a_call_times_out() {
        let mut config = server_answering_calls_with("echo stuck >&2");
        config.timeout = Some(1);
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        assert_eq!(
            mcp.call("mcp__test__echo", &json!({})).await,
            Some(Err(
                "tools/call timed out after 1 seconds\nstderr:\nstuck".into()
            ))
        );
    }

    #[tokio::test]
    async fn reaps_a_server_that_exits_mid_session() {
        let pid_file = std::env::temp_dir().join(format!("mcp-reap-{}", std::process::id()));
        let _ = std::fs::remove_file(&pid_file);
        let config =
            server_answering_calls_with(&format!("echo $$ > '{}'; exit 0", pid_file.display()));
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        assert!(
            mcp.call("mcp__test__echo", &json!({}))
                .await
                .unwrap()
                .is_err()
        );
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let _ = std::fs::remove_file(&pid_file);
        assert_eq!(
            wait_until_gone(pid.trim()).await,
            "",
            "server was not reaped"
        );
        drop(mcp);
    }

    #[tokio::test]
    async fn stops_what_a_server_left_running_when_it_exits() {
        let pid_file = std::env::temp_dir().join(format!("mcp-leftover-{}", std::process::id()));
        let _ = std::fs::remove_file(&pid_file);
        let config = server_answering_calls_with(&format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > '{}'; exit 0",
            pid_file.display()
        ));
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        assert!(
            mcp.call("mcp__test__echo", &json!({}))
                .await
                .unwrap()
                .is_err()
        );
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let _ = std::fs::remove_file(&pid_file);
        assert_eq!(
            wait_until_gone(pid.trim()).await,
            "",
            "leftover process kept running"
        );
        drop(mcp);
    }

    async fn wait_until_gone(pid: &str) -> String {
        let mut listed = String::new();
        for _ in 0..50 {
            let output = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .unwrap();
            listed = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if listed.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        listed
    }

    #[tokio::test]
    async fn stops_the_server_when_dropped() {
        let pid_file = std::env::temp_dir().join(format!("mcp-drop-{}", std::process::id()));
        let _ = std::fs::remove_file(&pid_file);
        let mut config =
            server_answering_calls_with(&format!("echo $$ > '{}'; sleep 30", pid_file.display()));
        config.timeout = Some(1);
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        let _ = mcp.call("mcp__test__echo", &json!({})).await;
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let _ = std::fs::remove_file(&pid_file);
        drop(mcp);
        assert_eq!(wait_until_gone(pid.trim()).await, "");
    }

    #[tokio::test]
    async fn tells_the_server_when_a_call_times_out() {
        let log = std::env::temp_dir().join(format!("mcp-cancel-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let mut config = server_with(
            &format!(
                r#"    *'"notifications/cancelled"'*) printf '%s\n' "$line" >> '{}' ;;"#,
                log.display()
            ),
            ":",
        );
        config.timeout = Some(1);
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        assert!(
            mcp.call("mcp__test__echo", &json!({}))
                .await
                .unwrap()
                .is_err()
        );
        let mut logged = String::new();
        for _ in 0..50 {
            logged = std::fs::read_to_string(&log).unwrap_or_default();
            if !logged.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = std::fs::remove_file(&log);
        let message: Value = serde_json::from_str(logged.trim()).unwrap();
        assert_eq!(message["method"], "notifications/cancelled");
        assert_eq!(message["params"]["requestId"], 3);
        assert!(message["params"]["reason"].is_string());
    }

    #[tokio::test]
    async fn times_out_when_the_server_stops_reading() {
        let mut config = server_answering_calls_with("sleep 30");
        config.timeout = Some(1);
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        let timed_out = Some(Err("tools/call timed out after 1 seconds".into()));
        assert_eq!(mcp.call("mcp__test__echo", &json!({})).await, timed_out);
        let large = json!({ "text": "a".repeat(1_000_000) });
        let call =
            tokio::time::timeout(Duration::from_secs(10), mcp.call("mcp__test__echo", &large));
        assert_eq!(call.await.ok(), Some(timed_out));
    }

    #[tokio::test]
    async fn keeps_working_after_a_large_call_is_cancelled_mid_write() {
        let mut mcp = Mcp::default();
        mcp.add(
            start(
                "test".into(),
                Scope::Project,
                server_answering_calls_with(
                    r#"case "$line" in *'"slow"'*) sleep 1 ;; esac
       id=${line#'{"id":'}; id=${id%%,*}
       printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
                ),
            )
            .await,
        );
        let cancel = Duration::from_millis(200);
        let slow = json!({ "slow": true });
        assert!(
            tokio::time::timeout(cancel, mcp.call("mcp__test__echo", &slow))
                .await
                .is_err()
        );
        let large = json!({ "text": "a".repeat(200_000) });
        assert!(
            tokio::time::timeout(cancel, mcp.call("mcp__test__echo", &large))
                .await
                .is_err()
        );
        assert_eq!(
            mcp.call("mcp__test__echo", &json!({})).await,
            Some(Ok("pong".into()))
        );
    }

    #[tokio::test]
    async fn skips_output_lines_that_are_not_utf8() {
        let mut mcp = Mcp::default();
        mcp.add(
            start(
                "test".into(),
                Scope::Project,
                server_answering_calls_with(
                    r#"printf '\377\n{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
                ),
            )
            .await,
        );
        assert_eq!(
            mcp.call("mcp__test__echo", &json!({})).await,
            Some(Ok("pong".into()))
        );
    }

    #[tokio::test]
    async fn answers_server_requests_and_follows_tool_list_pages() {
        let log = std::env::temp_dir().join(format!("mcp-replies-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let cases = r#"    *'"result"'*|*'"error"'*) printf '%s\n' "$line" >> 'LOG' ;;
    *'"cursor"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"second"}]}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"hi"}}\n'
      printf '{"jsonrpc":"2.0","id":"p1","method":"ping"}\n'
      printf '{"jsonrpc":"2.0","id":%s,"method":"sampling/createMessage","params":{}}\n' "$id"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo"}],"nextCursor":"next"}}\n' "$id" ;;"#
            .replace("LOG", &log.display().to_string());
        let mut mcp = Mcp::default();
        assert_eq!(
            mcp.add(start("test".into(), Scope::Project, server_with(&cases, ":")).await)
                .status,
            "loaded project MCP server: test (2 tools)"
        );
        let names: Vec<Value> = mcp
            .definitions()
            .map(|definition| definition["name"].clone())
            .collect();
        assert_eq!(names, ["mcp__test__echo", "mcp__test__second"]);
        let replies: Vec<Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let _ = std::fs::remove_file(&log);
        assert_eq!(
            replies,
            [
                json!({ "jsonrpc": "2.0", "id": "p1", "result": {} }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "error": {
                        "code": -32601,
                        "message": "method not found: sampling/createMessage",
                    },
                }),
            ]
        );
    }

    #[tokio::test]
    async fn calls_stdio_server_tools() {
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, echo_server()).await);
        assert_eq!(mcp.definitions().count(), 1);
        assert_eq!(
            mcp.call("mcp__test__echo", &json!({})).await,
            Some(Ok("pong".into()))
        );
        assert_eq!(mcp.call("bash", &json!({})).await, None);
    }

    #[tokio::test]
    async fn reports_each_server_as_it_starts() {
        let mut mcp = Mcp::default();
        let started = start("test".into(), Scope::Project, echo_server()).await;
        assert_eq!(
            mcp.add(started).status,
            "loaded project MCP server: test (1 tools)"
        );
        let mut broken = echo_server();
        broken.command = None;
        let failed = start("broken".into(), Scope::Global, broken).await;
        assert_eq!(
            mcp.add(failed).status,
            "MCP server broken failed: command is missing"
        );
        let mut lost = echo_server();
        lost.cwd = Some("/no/such/folder".into());
        let failed = start("lost".into(), Scope::Global, lost).await;
        assert_eq!(
            mcp.add(failed).status,
            "MCP server lost failed: cwd /no/such/folder is not a folder"
        );
        let clash = start("test".into(), Scope::Global, echo_server()).await;
        let added = mcp.add(clash);
        assert_eq!(
            added.diagnostics,
            ["MCP server test: skipped tool echo because its name clashes with test/echo"]
        );
        assert_eq!(added.status, "loaded global MCP server: test (0 tools)");
        assert!(mcp.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn fails_when_the_server_repeats_a_tool_list_cursor() {
        let config = server_with(
            r#"    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[],"nextCursor":"again"}}\n' "$id" ;;"#,
            ":",
        );
        let started = tokio::time::timeout(
            Duration::from_secs(10),
            start("test".into(), Scope::Project, config),
        )
        .await
        .expect("tools/list did not stop");
        assert_eq!(
            Mcp::default().add(started).status,
            "MCP server test failed: tools/list repeated the cursor \"again\""
        );
    }

    #[tokio::test]
    async fn fails_calls_when_the_server_sends_a_line_that_is_too_long() {
        let mut config = server_answering_calls_with(&format!(
            "head -c {} /dev/zero | tr '\\0' a; sleep 60",
            MAX_LINE + 1
        ));
        config.timeout = Some(30);
        let mut mcp = Mcp::default();
        mcp.add(start("test".into(), Scope::Project, config).await);
        let call = tokio::time::timeout(
            Duration::from_secs(20),
            mcp.call("mcp__test__echo", &json!({})),
        )
        .await;
        assert_eq!(
            call.ok(),
            Some(Some(Err(
                "server sent a line longer than 32 MiB; closed the connection".into()
            )))
        );
    }
}
