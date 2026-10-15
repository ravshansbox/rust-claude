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
