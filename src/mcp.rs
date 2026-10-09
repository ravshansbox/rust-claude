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
    process::{Child, ChildStdin},
    sync::oneshot,
};

use crate::{
    skills::Scope,
    tools::{self, ProcessGroup},
};

const PROTOCOL_VERSION: &str = "2025-06-18";
const DEFAULT_TIMEOUT: u64 = 60;
const STDERR_TAIL: usize = 4_000;
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
    enabled: Option<bool>,
    timeout: Option<u64>,
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

fn read_config(
    path: &Path,
    scope: Scope,
    servers: &mut BTreeMap<String, (Scope, ServerConfig)>,
    diagnostics: &mut Vec<String>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
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
        match serde_json::from_value(entry.clone()) {
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

fn config_paths() -> Vec<(Scope, PathBuf)> {
    let mut paths = Vec::new();
    if let Some(config_dir) = crate::config::dir() {
        paths.push((Scope::Global, config_dir.join("mcp.json")));
    }
    paths.push((
        Scope::Project,
        PathBuf::from(".rust-claude").join("mcp.json"),
    ));
    paths
}

struct Pending {
    senders: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    closed: AtomicBool,
}

struct PendingGuard<'a> {
    pending: &'a Pending,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut senders) = self.pending.senders.lock() {
            senders.remove(&self.id);
        }
    }
}

struct Connection {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Arc<Pending>,
    next_id: AtomicU64,
    stderr: Arc<Mutex<Vec<u8>>>,
    timeout: Duration,
    _child: Child,
    _process_group: ProcessGroup,
}

async fn write_message(
    stdin: &tokio::sync::Mutex<ChildStdin>,
    message: &Value,
) -> Result<(), String> {
    let mut line = message.to_string();
    line.push('\n');
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("failed to write to server: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("failed to write to server: {error}"))
}

async fn read_messages(
    stdout: impl tokio::io::AsyncRead + Unpin,
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Arc<Pending>,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = &message["id"];
        if let Some(method) = message["method"].as_str() {
            if id.is_null() {
                continue;
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
            let _ = write_message(&stdin, &reply).await;
            continue;
        }
        let Some(id) = id.as_u64() else {
            continue;
        };
        let sender = pending
            .senders
            .lock()
            .ok()
            .and_then(|mut senders| senders.remove(&id));
        if let Some(sender) = sender {
            let result = match message.get("error") {
                Some(error) => Err(format!(
                    "{} ({})",
                    error["message"].as_str().unwrap_or("unknown error"),
                    error["code"]
                )),
                None => Ok(message["result"].clone()),
            };
            let _ = sender.send(result);
        }
    }
    pending.closed.store(true, Ordering::SeqCst);
    if let Ok(mut senders) = pending.senders.lock() {
        senders.clear();
    }
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

impl Connection {
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
            process.current_dir(expand_home(cwd, home.as_deref()));
        }
        #[cfg(unix)]
        process.process_group(0);
        let mut child = process
            .spawn()
            .map_err(|error| format!("failed to start {command}: {error}"))?;
        let process_group = ProcessGroup(child.id());
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("failed to capture server input and output".into());
        };
        let stdin = Arc::new(tokio::sync::Mutex::new(stdin));
        let pending = Arc::new(Pending {
            senders: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(read_messages(stdout, stdin.clone(), pending.clone()));
        tokio::spawn(read_stderr(stderr, stderr_tail.clone()));
        Ok(Self {
            stdin,
            pending,
            next_id: AtomicU64::new(1),
            stderr: stderr_tail,
            timeout: Duration::from_secs(config.timeout.unwrap_or(DEFAULT_TIMEOUT)),
            _child: child,
            _process_group: process_group,
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

    async fn notify(&self, method: &str) -> Result<(), String> {
        write_message(&self.stdin, &json!({ "jsonrpc": "2.0", "method": method })).await
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
        };
        if self.pending.closed.load(Ordering::SeqCst) {
            return Err(self.with_stderr("server closed the connection".into()));
        }
        write_message(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await
        .map_err(|error| self.with_stderr(error))?;
        match tokio::time::timeout(self.timeout, receiver).await {
            Err(_) => Err(format!(
                "{method} timed out after {} seconds",
                self.timeout.as_secs()
            )),
            Ok(Err(_)) => Err(self.with_stderr("server closed the connection".into())),
            Ok(Ok(result)) => result,
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
        Value::Object(schema) => Value::Object(schema.clone()),
        _ => json!({}),
    };
    schema["type"] = json!("object");
    if !schema["properties"].is_object() {
        schema["properties"] = json!({});
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
    let mut cursor = Value::Null;
    loop {
        let params = if cursor.is_null() {
            json!({})
        } else {
            json!({ "cursor": cursor })
        };
        let result = connection.request("tools/list", params).await?;
        tools.extend(result["tools"].as_array().into_iter().flatten().cloned());
        cursor = result["nextCursor"].clone();
        if !cursor.is_string() {
            return Ok(tools);
        }
    }
}

async fn connect(name: String, scope: Scope, config: ServerConfig) -> Result<Server, String> {
    let command = match (config.kind.as_deref(), &config.command, &config.url) {
        (Some("sse"), _, _) => return Err("the SSE transport is not supported".into()),
        (Some("http" | "streamable-http"), _, _) | (None, None, Some(_)) => {
            return Err("HTTP servers are not supported yet".into());
        }
        (None | Some("stdio"), Some(command), _) => command.clone(),
        (None | Some("stdio"), None, _) => return Err("command is missing".into()),
        (Some(kind), _, _) => return Err(format!("unknown type {kind:?}")),
    };
    let connection = Connection::spawn(&config, &command)?;
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
    connection
        .notify("notifications/initialized")
        .await
        .map_err(|error| connection.with_stderr(error))?;
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
            Some("resource_link") => format!("[resource link: {}]", block["uri"]),
            Some("resource") => match block["resource"]["text"].as_str() {
                Some(text) => text.to_string(),
                None => format!("[resource: {}]", block["resource"]["uri"]),
            },
            _ => block.to_string(),
        });
    }
    if parts.is_empty() && !result["structuredContent"].is_null() {
        return result["structuredContent"].to_string();
    }
    parts.join("\n")
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

pub struct Startup {
    pub diagnostics: Vec<String>,
    pub servers: Vec<Starting>,
}

pub fn startup() -> Startup {
    let mut configs = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (scope, path) in config_paths() {
        read_config(&path, scope, &mut configs, &mut diagnostics);
    }
    let servers = configs
        .into_iter()
        .filter(|(_, (_, config))| config.enabled != Some(false))
        .map(|(name, (scope, config))| Box::pin(start(name, scope, config)) as Starting)
        .collect();
    Startup {
        diagnostics,
        servers,
    }
}

impl Mcp {
    pub async fn load() -> Self {
        let startup = startup();
        let mut mcp = Self {
            servers: Vec::new(),
            diagnostics: startup.diagnostics,
        };
        for started in futures::future::join_all(startup.servers).await {
            let (diagnostics, _) = mcp.add_server(started);
            mcp.diagnostics.extend(diagnostics);
        }
        mcp
    }

    pub fn add(&mut self, started: Started) -> Vec<String> {
        let (mut messages, loaded) = self.add_server(started);
        messages.extend(loaded);
        messages
    }

    fn add_server(&mut self, started: Started) -> (Vec<String>, Option<String>) {
        let Started { name, result } = started;
        let mut server = match result {
            Ok(server) => server,
            Err(error) => return (vec![format!("MCP server {name} failed: {error}")], None),
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
        (diagnostics, Some(loaded))
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
    fn converts_results_to_text() {
        let result = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "image", "data": "", "mimeType": "image/png" },
                { "type": "resource", "resource": { "uri": "file:///a", "text": "body" } },
            ]
        });
        assert_eq!(result_text(&result), "hello\n[image: image/png]\nbody");
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

    fn echo_server() -> ServerConfig {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"t","version":"1"}}}\n' "$id" ;;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id" ;;
  esac
done
"#;
        ServerConfig {
            kind: None,
            command: Some("bash".into()),
            args: vec!["-c".into(), script.into()],
            env: HashMap::new(),
            cwd: None,
            url: None,
            enabled: None,
            timeout: Some(5),
        }
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
            mcp.add(started),
            ["loaded project MCP server: test (1 tools)"]
        );
        let mut broken = echo_server();
        broken.command = None;
        let failed = start("broken".into(), Scope::Global, broken).await;
        assert_eq!(
            mcp.add(failed),
            ["MCP server broken failed: command is missing"]
        );
        let clash = start("test".into(), Scope::Global, echo_server()).await;
        assert_eq!(
            mcp.add(clash),
            [
                "MCP server test: skipped tool echo because its name clashes with test/echo",
                "loaded global MCP server: test (0 tools)",
            ]
        );
        assert!(mcp.diagnostics.is_empty());
    }
}
