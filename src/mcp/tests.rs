use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::config_file::{OAuthConfig, expand_variables, read_config};
use super::*;
use crate::auth::now_millis;
use crate::tools::test_support::{TemporaryDir, TemporaryFile};

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
fn leaves_an_unclosed_variable_as_written() {
    assert_eq!(
        expand_variables("Bearer ${TOKEN"),
        Ok("Bearer ${TOKEN".into())
    );
}

#[test]
fn makes_object_schemas() {
    let definition = tool_definition(
        "mcp__docs__search",
        json!({ "name": "search", "inputSchema": { "type": "object" } }),
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
        json!({
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
    let directory = TemporaryDir::new("mcp-test");
    let global = directory.path().join("global.json");
    let project = directory.path().join("project.json");
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
    assert_eq!(servers.len(), 1);
    assert_eq!(servers["docs"].0, Scope::Project);
    assert_eq!(servers["docs"].1.command.as_deref(), Some("c"));
    assert_eq!(diagnostics.len(), 1);
}

#[test]
fn reads_oauth_settings_from_global_and_project_configs() {
    let file = TemporaryFile::new(
        "mcp-oauth",
        r#"{ "mcpServers": { "figma": { "url": "https://mcp.figma.com/mcp", "oauth": { "clientId": "own" } } } }"#,
    );
    let mut servers = BTreeMap::new();
    let mut diagnostics = Vec::new();
    read_config(file.path(), Scope::Project, &mut servers, &mut diagnostics);
    assert!(diagnostics.is_empty());
    assert_eq!(servers["figma"].0, Scope::Project);
    assert_eq!(
        servers["figma"]
            .1
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.client_id.as_deref()),
        Some("own")
    );
}

#[test]
fn rejects_a_zero_timeout() {
    let file = TemporaryFile::new(
        "mcp-timeout",
        r#"{ "mcpServers": { "docs": { "command": "a", "timeout": 0 } } }"#,
    );
    let mut servers = BTreeMap::new();
    let mut diagnostics = Vec::new();
    read_config(file.path(), Scope::Project, &mut servers, &mut diagnostics);
    assert!(servers.is_empty());
    assert_eq!(diagnostics, ["MCP server docs: timeout must be at least 1"]);
}

#[test]
fn reports_invalid_server_entries_and_keeps_the_rest() {
    let global = TemporaryFile::new(
        "mcp-invalid-global",
        r#"{ "mcpServers": { "docs": { "command": "a" } } }"#,
    );
    let project = TemporaryFile::new(
        "mcp-invalid-project",
        r#"{ "mcpServers": { "bad name": { "command": "a" }, "docs": { "command": 5 }, "ok": { "command": "b" } } }"#,
    );
    let mut servers = BTreeMap::new();
    let mut diagnostics = Vec::new();
    read_config(global.path(), Scope::Global, &mut servers, &mut diagnostics);
    read_config(
        project.path(),
        Scope::Project,
        &mut servers,
        &mut diagnostics,
    );
    assert_eq!(servers.keys().collect::<Vec<_>>(), ["ok"]);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0],
        "MCP: server name \"bad name\" may only contain letters, digits, _ and -"
    );
    assert!(
        diagnostics[1].starts_with("MCP server docs: invalid entry: "),
        "{diagnostics:?}"
    );
}

#[test]
fn reports_mcp_servers_that_are_not_an_object() {
    let file = TemporaryFile::new("mcp-array", r#"{ "mcpServers": [] }"#);
    let mut servers = BTreeMap::new();
    let mut diagnostics = Vec::new();
    read_config(file.path(), Scope::Project, &mut servers, &mut diagnostics);
    assert!(servers.is_empty());
    assert_eq!(
        diagnostics,
        [format!(
            "MCP: mcpServers in {} must be an object",
            file.path().display()
        )]
    );
}

#[test]
fn reports_an_unreadable_config() {
    let directory = TemporaryDir::new("mcp-unreadable");
    let path = directory.path();
    let mut servers = BTreeMap::new();
    let mut diagnostics = Vec::new();
    read_config(path, Scope::Project, &mut servers, &mut diagnostics);
    read_config(
        &path.join("missing.json"),
        Scope::Project,
        &mut servers,
        &mut diagnostics,
    );
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].starts_with(&format!("MCP: cannot read {}: ", path.display())));
}

#[test]
fn labels_servers_global_when_run_from_the_home_folder() {
    let directory = TemporaryDir::new("mcp-home");
    let home = directory.path();
    std::fs::create_dir(home.join(".rust-coding-agent")).unwrap();
    std::fs::write(
        home.join(".rust-coding-agent/mcp.json"),
        r#"{ "mcpServers": { "docs": { "command": "a" } } }"#,
    )
    .unwrap();
    let (servers, overridden, diagnostics) =
        read_configs(Some(&home.join(".rust-coding-agent")), home);
    assert_eq!(servers["docs"].0, Scope::Global);
    assert!(overridden.is_empty());
    assert!(diagnostics.is_empty());
}

#[test]
fn lists_enabled_global_servers_that_project_entries_override() {
    let directory = TemporaryDir::new("mcp-override");
    let global = directory.path().join("global");
    let project = directory.path().join("project");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(project.join(".rust-coding-agent")).unwrap();
    std::fs::write(
        global.join("mcp.json"),
        r#"{ "mcpServers": {
                "docs": { "command": "a" },
                "off": { "command": "a", "enabled": false },
                "web": { "command": "a" }
            } }"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".rust-coding-agent/mcp.json"),
        r#"{ "mcpServers": { "docs": { "command": "c" }, "off": { "command": "c" } } }"#,
    )
    .unwrap();
    let (servers, overridden, _) = read_configs(Some(&global), &project);
    assert_eq!(servers["docs"].0, Scope::Project);
    assert_eq!(overridden, ["docs"]);
}

#[test]
fn restarts_only_enabled_servers() {
    let temporary = TemporaryDir::new("mcp-restart");
    let directory = temporary.path();
    std::fs::write(
        directory.join("mcp.json"),
        r#"{ "mcpServers": {
                "docs": { "command": "a" },
                "off": { "command": "a", "enabled": false }
            } }"#,
    )
    .unwrap();
    let configs = || read_configs(Some(directory), directory).0;
    let docs = restart_from(configs(), "docs").map(|server| server.name);
    let off = restart_from(configs(), "off").map(|server| server.name);
    assert_eq!(docs.as_deref(), Some("docs"));
    assert_eq!(off.as_deref(), None);
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
        command: Some("bash".into()),
        args: vec!["-c".into(), script],
        timeout: Some(5),
        ..Default::default()
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
                    let _ = write!(response, "{name}: {value}\r\n");
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
    let head = String::from_utf8_lossy(&data[..header_end]);
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
        url: Some(url.into()),
        timeout: Some(5),
        ..Default::default()
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
async fn lists_global_server_tools_before_project_ones() {
    let (url, _) = http_server(http_mcp).await;
    let mut mcp = Mcp::default();
    for (name, scope) in [
        ("alpha", Scope::Project),
        ("zeta", Scope::Global),
        ("beta", Scope::Global),
    ] {
        mcp.add(start(name.into(), scope, http_config(&url)).await);
    }
    let names: Vec<Value> = mcp
        .definitions()
        .map(|definition| definition["name"].clone())
        .collect();
    assert_eq!(
        names,
        ["mcp__beta__echo", "mcp__zeta__echo", "mcp__alpha__echo"]
    );
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
    let replies = || -> Vec<Value> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.body["id"] == "p1")
            .map(|request| request.body.clone())
            .collect()
    };
    let replies = eventually(replies, |replies| !replies.is_empty()).await;
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
        "Bearer ${RUST_CODING_AGENT_UNSET_TOKEN}".into(),
    );
    assert_eq!(
        mcp.add(start("unset".into(), Scope::Global, config).await)
            .status,
        "MCP server unset failed: header Authorization uses RUST_CODING_AGENT_UNSET_TOKEN, which is not set"
    );
}

#[tokio::test]
async fn sends_headers_from_the_headers_helper() {
    let (url, requests) = http_server(http_mcp).await;
    let mut config = http_config(&url);
    config.headers.insert("X-Static".into(), "kept".into());
    config
        .headers
        .insert("Authorization".into(), "Bearer static".into());
    config.headers_helper = Some(
            r#"printf '{"Authorization": "Bearer %s %s"}' "$CLAUDE_CODE_MCP_SERVER_NAME" "$CLAUDE_CODE_MCP_SERVER_URL""#
                .into(),
        );
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
    for method in ["initialize", "tools/call"] {
        let request = &requests_for(&requests, method)[0];
        assert_eq!(
            request.headers["authorization"],
            format!("Bearer web {url}")
        );
        assert_eq!(request.headers["x-static"], "kept");
    }
}

#[tokio::test]
async fn reports_a_failing_headers_helper() {
    let (url, _) = http_server(http_mcp).await;
    let mut mcp = Mcp::default();
    for (helper, reason) in [
        (
            "echo oops >&2; exit 3",
            "headersHelper exited with status 3: oops",
        ),
        (
            "echo '[1]'",
            "headersHelper must print a JSON object of strings",
        ),
        (
            r#"echo '{"A": 1}'"#,
            "headersHelper must print a JSON object of strings",
        ),
        (
            r#"echo '{"A B": "c"}'"#,
            "headersHelper gave invalid header name \"A B\"",
        ),
    ] {
        let mut config = http_config(&url);
        config.headers_helper = Some(helper.into());
        assert_eq!(
            mcp.add(start("web".into(), Scope::Global, config).await)
                .status,
            format!("MCP server web failed: {reason}"),
            "{helper}"
        );
    }
}

#[tokio::test]
async fn does_not_sign_in_with_oauth_when_a_headers_helper_is_set() {
    let (url, _) = http_server(|_| HttpReply::status(401, "")).await;
    let mut config = http_config(&url);
    config.headers_helper = Some("echo '{}'".into());
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, config).await)
            .status,
        "MCP server web failed: server answered initialize with status 401"
    );
}

#[tokio::test]
async fn runs_the_headers_helper_again_after_401_and_for_a_new_session() {
    let counter = TemporaryFile::new("mcp-helper-count", "0");
    let sessions = Arc::new(AtomicU64::new(0));
    let issued = sessions.clone();
    let (url, requests) = http_server(move |request| {
        let session = request.headers.get("mcp-session-id").map(String::as_str);
        let token = request.headers.get("authorization").map(String::as_str);
        match request.body["method"].as_str() {
            Some("initialize") => {
                let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
                http_mcp(request).with_header("mcp-session-id", &format!("s{number}"))
            }
            Some("tools/call") if token == Some("Bearer t1") => HttpReply::status(401, ""),
            Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
            _ => http_mcp(request),
        }
    })
    .await;
    let mut config = http_config(&url);
    config.headers_helper = Some(format!(
        r#"n=$(($(cat '{0}' 2>/dev/null || echo 0) + 1)); echo $n > '{0}'; printf '{{"Authorization": "Bearer t%s"}}' $n"#,
        counter.path().display()
    ));
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, config).await);
    assert_eq!(
        mcp.call("mcp__web__echo", &json!({})).await,
        Some(Ok("pong".into()))
    );
    let tokens = |method: &str| -> Vec<String> {
        requests_for(&requests, method)
            .iter()
            .map(|request| request.headers["authorization"].clone())
            .collect()
    };
    assert_eq!(tokens("initialize"), ["Bearer t1", "Bearer t3"]);
    assert_eq!(
        tokens("tools/call"),
        ["Bearer t1", "Bearer t2", "Bearer t3"]
    );
}

#[tokio::test]
async fn retries_connecting_after_a_transient_http_error() {
    let attempts = Arc::new(AtomicU64::new(0));
    let counted = attempts.clone();
    let (url, requests) = http_server(move |request| {
        if request.body["method"] == "initialize" && counted.fetch_add(1, Ordering::SeqCst) < 2 {
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

#[tokio::test]
async fn tries_a_new_http_session_again_after_a_failed_renewal() {
    let sessions = Arc::new(AtomicU64::new(0));
    let issued = sessions.clone();
    let (url, _requests) = http_server(move |request| {
        let session = request.headers.get("mcp-session-id").map(String::as_str);
        match request.body["method"].as_str() {
            Some("initialize") => match issued.fetch_add(1, Ordering::SeqCst) + 1 {
                2 => HttpReply::status(503, ""),
                number => http_mcp(request).with_header("mcp-session-id", &format!("s{number}")),
            },
            _ if session.is_none() => HttpReply::status(400, ""),
            Some("tools/call") if session == Some("s1") => HttpReply::status(404, ""),
            _ => http_mcp(request),
        }
    })
    .await;
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert!(matches!(
        mcp.call("mcp__web__echo", &json!({})).await,
        Some(Err(_))
    ));
    assert_eq!(
        mcp.call("mcp__web__echo", &json!({})).await,
        Some(Ok("pong".into()))
    );
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
            secret_from_config: false,
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
async fn sends_the_client_secret_from_the_config_when_renewing() {
    let (url, requests) = http_server(signed_in_mcp).await;
    save_tokens(&url, "stale", now_millis() - 1);
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("client".into()),
        client_secret: Some("own-secret".into()),
    });
    let mut mcp = Mcp::default();
    assert_eq!(
        mcp.add(start("web".into(), Scope::Global, config).await)
            .status,
        "loaded global MCP server: web (1 tools)"
    );
    let token_form = requests_for_path(&requests, "/token")[0].form();
    assert_eq!(token_form["client_secret"], "own-secret");
    let file = std::fs::read_to_string(config::dir().unwrap().join("mcp-auth.json")).unwrap();
    assert!(!file.contains("own-secret"), "{file}");
}

#[tokio::test]
async fn keeps_the_client_secret_from_the_config_when_taking_over_tokens() {
    let (url, _) = http_server(signed_in_mcp).await;
    save_tokens(&url, "mine", now_millis() - 1);
    let config = OAuthConfig {
        client_id: Some("client".into()),
        client_secret: Some("own-secret".into()),
    };
    let mine = oauth::load_for(&url, Some(&config)).unwrap();
    save_tokens(&url, "theirs", now_millis() + 3_600_000);
    let taken_over = oauth::refresh(&reqwest::Client::new(), &url, &mine, None)
        .await
        .unwrap();
    assert_eq!(taken_over.access_token, "theirs");
    assert_eq!(taken_over.client_secret.as_deref(), Some("own-secret"));
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
async fn uses_short_lived_oauth_tokens_until_they_expire() {
    let (url, requests) = http_server(|request| {
        if request.path == "/token" {
            return HttpReply::json(json!({
                "access_token": "fresh",
                "token_type": "Bearer",
                "expires_in": 30,
            }));
        }
        signed_in_mcp(request)
    })
    .await;
    save_tokens(&url, "stale", now_millis() - 1);
    let mut mcp = Mcp::default();
    mcp.add(start("web".into(), Scope::Global, http_config(&url)).await);
    assert_eq!(
        mcp.call("mcp__web__echo", &json!({})).await,
        Some(Ok("pong".into()))
    );
    assert_eq!(requests_for_path(&requests, "/token").len(), 1);
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
    use base64::Engine;
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

#[tokio::test]
async fn signs_in_while_another_connection_to_the_callback_stays_idle() {
    let (url, _) = oauth_server().await;
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let callback = reqwest::Url::parse(&query(&authorize_url)["redirect_uri"]).unwrap();
    let _idle = tokio::net::TcpStream::connect(("127.0.0.1", callback.port().unwrap()))
        .await
        .unwrap();
    let finished = tokio::spawn(sign_in.finish());
    let page = tokio::time::timeout(
        Duration::from_secs(10),
        visit_callback(&authorize_url, "the-code", None),
    )
    .await
    .expect("the idle connection stalled the sign-in");
    assert!(page.contains("Signed in"), "{page}");
    assert_eq!(finished.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn reads_sign_in_parameters_outside_quoted_values() {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
            let base = shared.lock().unwrap().clone();
            let reply = oauth_mcp(&base, request);
            if reply.status != 401 {
                return reply;
            }
            HttpReply::status(401, "").with_header(
                "www-authenticate",
                &format!(
                    r#"Bearer error="insufficient_scope", error_description="needs scope=admin, see resource_metadata=https://elsewhere.example", resource_metadata="{base}/.well-known/oauth-protected-resource/mcp", scope="mcp:connect""#
                ),
            )
        })
        .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    assert_eq!(query(&sign_in.authorize_url)["scope"], "mcp:connect");
}

#[tokio::test]
async fn refuses_sign_in_endpoints_without_https() {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        if request.path == "/.well-known/oauth-authorization-server/auth" {
            return HttpReply::json(json!({
                "authorization_endpoint": "https://sign-in.example/authorize",
                "token_endpoint": "http://sign-in.example/token",
                "registration_endpoint": format!("{base}/register"),
            }));
        }
        oauth_mcp(&base, request)
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(
            "the sign-in server's token endpoint does not use HTTPS: http://sign-in.example/token"
                .into()
        )
    );
}

#[tokio::test]
async fn refuses_to_sign_in_to_a_server_without_https() {
    assert_eq!(
        begin_sign_in_with("web", &http_config("http://mcp.example/mcp"))
            .await
            .err(),
        Some("the MCP server does not use HTTPS: http://mcp.example/mcp".into())
    );
}

/// Serves `oauth_mcp`, with `change` applied to the JSON it sends for
/// `path`.
async fn oauth_server_changing(
    path: &'static str,
    change: impl Fn(&mut Value) + Send + Sync + 'static,
) -> String {
    let base = Arc::new(Mutex::new(String::new()));
    let shared = base.clone();
    let (url, _) = http_server(move |request| {
        let base = shared.lock().unwrap().clone();
        let mut reply = oauth_mcp(&base, request);
        if request.path == path {
            let mut metadata: Value = serde_json::from_str(&reply.body).unwrap();
            change(&mut metadata);
            reply = HttpReply::json(metadata);
        }
        reply
    })
    .await;
    *base.lock().unwrap() = url.trim_end_matches("/mcp").to_string();
    url
}

#[tokio::test]
async fn refuses_resource_metadata_for_another_server() {
    let url = oauth_server_changing("/.well-known/oauth-protected-resource/mcp", |metadata| {
        metadata["resource"] = json!("https://other.example/mcp");
    })
    .await;
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(format!(
            "the sign-in metadata of {url} is for another server: https://other.example/mcp"
        ))
    );
}

#[tokio::test]
async fn accepts_resource_metadata_for_the_server_origin() {
    let url = oauth_server_changing("/.well-known/oauth-protected-resource/mcp", |metadata| {
        let resource = metadata["resource"]
            .as_str()
            .unwrap()
            .trim_end_matches("/mcp")
            .to_string();
        metadata["resource"] = json!(resource);
    })
    .await;
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    assert_eq!(
        query(&sign_in.authorize_url)["resource"],
        url.trim_end_matches("/mcp")
    );
}

#[tokio::test]
async fn refuses_a_sign_in_server_that_names_another_issuer() {
    let url = oauth_server_changing("/.well-known/oauth-authorization-server/auth", |metadata| {
        metadata["issuer"] = json!("https://other.example");
    })
    .await;
    let base = url.trim_end_matches("/mcp");
    assert_eq!(
        begin_sign_in_with("web", &http_config(&url)).await.err(),
        Some(format!(
            "the sign-in server {base}/auth names another issuer: https://other.example"
        ))
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
async fn does_not_save_the_client_secret_from_the_config() {
    let (url, requests) = oauth_server().await;
    let mut config = http_config(&url);
    config.oauth = Some(OAuthConfig {
        client_id: Some("own".into()),
        client_secret: Some("own-secret".into()),
    });
    let sign_in = begin_sign_in_with("web", &config).await.unwrap();
    let authorize_url = sign_in.authorize_url.clone();
    let finished = tokio::spawn(sign_in.finish());
    visit_callback(&authorize_url, "the-code", None).await;
    assert_eq!(finished.await.unwrap(), Ok(()));
    assert_eq!(
        requests_for_path(&requests, "/token")[0].form()["client_secret"],
        "own-secret"
    );
    let file = std::fs::read_to_string(config::dir().unwrap().join("mcp-auth.json")).unwrap();
    assert!(!file.contains("own-secret"), "{file}");
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
    let sign_in = begin_sign_in_with("web", &http_config(&url)).await.unwrap();
    let mut callback = reqwest::Url::parse(&query(&sign_in.authorize_url)["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("state", "forged");
    let finished = tokio::spawn(sign_in.finish());
    reqwest::get(callback).await.unwrap();
    assert_eq!(
        finished.await.unwrap(),
        Err("the sign-in page sent back a different state".into())
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
async fn rejects_urls_that_are_not_http() {
    let mut mcp = Mcp::default();
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
    let deleted = || -> Vec<String> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == "DELETE")
            .map(|request| request.headers["mcp-session-id"].clone())
            .collect()
    };
    let deleted = eventually(deleted, |deleted| !deleted.is_empty()).await;
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
async fn keeps_whole_characters_in_the_end_of_server_errors() {
    let mut config = server_answering_calls_with(&format!("echo '{}' >&2", "é".repeat(2_000)));
    config.timeout = Some(1);
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, config).await);
    assert_eq!(
        mcp.call("mcp__test__echo", &json!({})).await,
        Some(Err(format!(
            "tools/call timed out after 1 seconds\nstderr:\n{}",
            "é".repeat(1_999)
        )))
    );
}

#[tokio::test]
async fn reaps_a_server_that_exits_mid_session() {
    let pid_file = TemporaryFile::new("mcp-reap", "");
    let config = server_answering_calls_with(&format!(
        "echo $$ > '{}'; exit 0",
        pid_file.path().display()
    ));
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, config).await);
    assert!(
        mcp.call("mcp__test__echo", &json!({}))
            .await
            .unwrap()
            .is_err()
    );
    let pid = pid_file.content();
    assert_eq!(
        wait_until_gone(pid.trim()).await,
        "",
        "server was not reaped"
    );
    drop(mcp);
}

#[tokio::test]
async fn stops_what_a_server_left_running_when_it_exits() {
    let pid_file = TemporaryFile::new("mcp-leftover", "");
    let config = server_answering_calls_with(&format!(
        "sleep 30 >/dev/null 2>&1 & echo $! > '{}'; exit 0",
        pid_file.path().display()
    ));
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, config).await);
    assert!(
        mcp.call("mcp__test__echo", &json!({}))
            .await
            .unwrap()
            .is_err()
    );
    let pid = pid_file.content();
    assert_eq!(
        wait_until_gone(pid.trim()).await,
        "",
        "leftover process kept running"
    );
    drop(mcp);
}

async fn eventually<T>(mut value: impl FnMut() -> T, done: impl Fn(&T) -> bool) -> T {
    for _ in 1..50 {
        let current = value();
        if done(&current) {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    value()
}

async fn wait_until_gone(pid: &str) -> String {
    let listed = || {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    eventually(listed, String::is_empty).await
}

#[tokio::test]
async fn stops_the_server_when_dropped() {
    let pid_file = TemporaryFile::new("mcp-drop", "");
    let mut config = server_answering_calls_with(&format!(
        "echo $$ > '{}'; sleep 30",
        pid_file.path().display()
    ));
    config.timeout = Some(1);
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, config).await);
    let _ = mcp.call("mcp__test__echo", &json!({})).await;
    let pid = pid_file.content();
    drop(mcp);
    assert_eq!(wait_until_gone(pid.trim()).await, "");
}

#[tokio::test]
async fn tells_the_server_when_a_call_times_out() {
    let log = TemporaryFile::new("mcp-cancel", "");
    let mut config = server_with(
        &format!(
            r#"    *'"notifications/cancelled"'*) printf '%s\n' "$line" >> '{}' ;;"#,
            log.path().display()
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
    let logged = eventually(|| log.content(), |logged| !logged.is_empty()).await;
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
    let call = tokio::time::timeout(Duration::from_secs(10), mcp.call("mcp__test__echo", &large));
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
    let log = TemporaryFile::new("mcp-replies", "");
    let cases = r#"    *'"result"'*|*'"error"'*) printf '%s\n' "$line" >> 'LOG' ;;
    *'"cursor"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"second"}]}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"hi"}}\n'
      printf '{"jsonrpc":"2.0","id":"p1","method":"ping"}\n'
      printf '{"jsonrpc":"2.0","id":%s,"method":"sampling/createMessage","params":{}}\n' "$id"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo"}],"nextCursor":"next"}}\n' "$id" ;;"#
            .replace("LOG", &log.path().display().to_string());
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
    let replies: Vec<Value> = log
        .content()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
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
async fn reports_tool_calls_the_server_marks_as_errors() {
    let server = server_answering_calls_with(
        r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"isError":true,"content":[{"type":"text","text":"no such page"}]}}\n' "$id""#,
    );
    let mut mcp = Mcp::default();
    mcp.add(start("test".into(), Scope::Project, server).await);
    assert_eq!(
        mcp.call("mcp__test__echo", &json!({})).await,
        Some(Err("no such page".into()))
    );
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

#[test]
fn shortens_the_load_notice_for_servers_that_need_sign_in() {
    let mcp = Mcp::from_started(
        vec!["MCP: invalid mcp.json".into()],
        [
            failed_start("figma", "needs sign-in: run /mcp login figma"),
            failed_start("broken", "command is missing"),
        ],
    );
    assert_eq!(
        mcp.diagnostics,
        [
            "MCP: invalid mcp.json",
            "figma (needs sign-in)",
            "MCP server broken failed: command is missing"
        ]
    );
}

#[tokio::test]
async fn sums_up_each_server_in_short() {
    let mut mcp = Mcp::default();
    let added = mcp.add(start("test".into(), Scope::Project, echo_server()).await);
    assert_eq!(
        (added.summary.as_str(), added.failed),
        ("test (1 tools)", false)
    );
    let added = mcp.add(failed_start("broken", "command is missing"));
    assert_eq!(
        (added.summary.as_str(), added.failed),
        ("broken (failed)", true)
    );
    let added = mcp.add(failed_start("figma", "needs sign-in: run /mcp login figma"));
    assert_eq!(
        (added.summary.as_str(), added.failed),
        ("figma (needs sign-in)", true)
    );
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
