use super::*;

/// Answers like a Streamable HTTP server that starts session `s1`.
pub(super) fn http_mcp(request: &HttpRequest) -> HttpReply {
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

pub(super) fn http_config(url: &str) -> ServerConfig {
    ServerConfig {
        url: Some(url.into()),
        timeout: Some(5),
        ..Default::default()
    }
}

pub(super) async fn call_web_echo(mcp: &Mcp) -> Option<Result<String, String>> {
    mcp.call("mcp__web__echo", &json!({})).await
}

pub(super) fn requests_for(requests: &Requests, method: &str) -> Vec<HttpRequest> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.body["method"] == method)
        .cloned()
        .collect()
}
