use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::io::AsyncWriteExt;

use super::config_file::{OAuthConfig, expand_variables, read_config};
use super::*;
use crate::agent::test_support::read_chunk;
use crate::auth::now_millis;
use crate::tools::test_support::{TemporaryDir, TemporaryFile};

mod config_files;
mod http;
mod sign_in;
mod stdio;

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
    fn json(body: &Value) -> Self {
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
    let header_end = loop {
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        read_chunk(stream, &mut data).await?;
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
        read_chunk(stream, &mut data).await?;
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
        Some("initialize") => HttpReply::json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "t", "version": "1" },
            },
        }))
        .with_header("mcp-session-id", "s1"),
        Some("tools/list") => HttpReply::json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] },
        })),
        Some("tools/call") => HttpReply::json(&json!({
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
