use super::*;

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
fn reports_a_config_file_that_is_not_valid_json_and_keeps_other_files() {
    let global = TemporaryFile::new(
        "mcp-valid-global",
        r#"{ "mcpServers": { "docs": { "command": "a" } } }"#,
    );
    let project = TemporaryFile::new("mcp-truncated-project", r#"{ "mcpServers": "#);
    let mut servers = BTreeMap::new();
    let mut diagnostics = Vec::new();
    read_config(global.path(), Scope::Global, &mut servers, &mut diagnostics);
    read_config(
        project.path(),
        Scope::Project,
        &mut servers,
        &mut diagnostics,
    );
    assert_eq!(servers.keys().collect::<Vec<_>>(), ["docs"]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].starts_with(&format!("MCP: invalid {}: ", project.path().display())),
        "{diagnostics:?}"
    );
}

#[tokio::test]
async fn reports_transports_it_cannot_use() {
    let mut mcp = Mcp::default();
    for (config, status) in [
        (
            json!({ "type": "sse", "url": "http://127.0.0.1:1/sse" }),
            "the SSE transport is not supported; use the server's Streamable HTTP URL",
        ),
        (json!({ "type": "http" }), "url is missing"),
        (
            json!({ "type": "websocket", "url": "ws://127.0.0.1:1" }),
            "unknown type \"websocket\"",
        ),
    ] {
        let config: ServerConfig = serde_json::from_value(config).unwrap();
        let failed = start("remote".into(), Scope::Project, config).await;
        assert_eq!(
            mcp.add(failed).status,
            format!("MCP server remote failed: {status}")
        );
    }
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
