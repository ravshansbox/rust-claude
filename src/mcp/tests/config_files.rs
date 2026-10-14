use super::*;

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
    assert_eq!(diagnostics, Vec::<String>::new());
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
    assert_eq!(overridden, Vec::<String>::new());
    assert_eq!(diagnostics, Vec::<String>::new());
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
