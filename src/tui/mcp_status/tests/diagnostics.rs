use super::*;

#[test]
fn shows_why_an_mcp_server_failed_below_its_group() {
    let mut app = new_app();
    app.start_mcp_servers([("docs", Scope::Project)], []);
    handle_agent_event(
        mcp_server_failed(
            "docs",
            "docs (failed)",
            "MCP server docs failed: connection refused",
        ),
        &mut app,
    );
    let shown = screen(&mut app);
    let group = shown
        .find("loaded project MCP servers: docs (failed)")
        .expect(&shown);
    let reason = shown
        .find("MCP server docs failed: connection refused")
        .expect(&shown);
    assert!(group < reason, "{shown}");
}

#[test]
fn shows_mcp_server_warnings_below_its_group() {
    let mut app = new_app();
    app.start_mcp_servers([("docs", Scope::Project)], []);
    handle_agent_event(
        UiEvent::McpServer(mcp::Added {
            name: "docs".into(),
            status: "loaded MCP server: docs (1 tool)".into(),
            summary: "docs (1 tool)".into(),
            failed: false,
            diagnostics: vec![
                "MCP server docs: skipped tool echo because its name clashes with web/echo".into(),
            ],
        }),
        &mut app,
    );
    let shown = screen(&mut app);
    let group = shown
        .find("loaded project MCP servers: docs (1 tool)")
        .expect(&shown);
    let warning = shown
        .find("MCP server docs: skipped tool echo because its name clashes with web/echo")
        .expect(&shown);
    assert!(group < warning, "{shown}");
}
