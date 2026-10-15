use super::Scope;
use crate::mcp;
use crate::tui::{
    Role, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, screen},
};

mod diagnostics;

fn mcp_server_loaded(name: &str, summary: &str) -> UiEvent {
    UiEvent::McpServer(mcp::Added {
        name: name.into(),
        status: format!("loaded MCP server: {summary}"),
        summary: summary.into(),
        failed: false,
        diagnostics: Vec::new(),
    })
}

fn mcp_server_failed(name: &str, summary: &str, status: &str) -> UiEvent {
    UiEvent::McpServer(mcp::Added {
        name: name.into(),
        status: status.into(),
        summary: summary.into(),
        failed: true,
        diagnostics: Vec::new(),
    })
}

fn mcp_server_restarting(name: &str, scope: Scope) -> UiEvent {
    UiEvent::McpRestarting {
        name: name.into(),
        scope,
        notice: format!("signed in to MCP server {name}"),
    }
}

#[test]
fn groups_mcp_servers_by_scope_on_two_lines_with_spinners() {
    let mut app = new_app();
    app.start_mcp_servers(
        [
            ("docs", Scope::Project),
            ("figma", Scope::Global),
            ("web", Scope::Global),
        ],
        [],
    );
    app.push(Role::User, "hello");
    let shown = screen(&mut app);
    let global = shown
        .find("global MCP servers: ⠋ figma, ⠋ web")
        .expect(&shown);
    let project = shown.find("project MCP servers: ⠋ docs").expect(&shown);
    let hello = shown.find("hello").expect(&shown);
    assert!(global < project && project < hello, "{shown}");
    handle_agent_event(mcp_server_loaded("web", "web (3 tools)"), &mut app);
    let shown = screen(&mut app);
    assert!(
        shown.contains("global MCP servers: ⠋ figma, web (3 tools)"),
        "{shown}"
    );
    handle_agent_event(
        mcp_server_failed(
            "figma",
            "figma (needs sign-in)",
            "MCP server figma needs sign-in: run /mcp login figma",
        ),
        &mut app,
    );
    let shown = screen(&mut app);
    let global = shown
        .find("loaded global MCP servers: figma (needs sign-in), web (3 tools)")
        .expect(&shown);
    let project = shown.find("project MCP servers: ⠋ docs").expect(&shown);
    let hello = shown.find("hello").expect(&shown);
    assert!(global < project && project < hello, "{shown}");
    assert!(!shown.contains("MCP server figma needs sign-in"), "{shown}");
}

#[test]
fn shows_global_mcp_servers_that_project_entries_override() {
    let mut app = new_app();
    app.start_mcp_servers([("docs", Scope::Project), ("web", Scope::Global)], ["docs"]);
    let shown = screen(&mut app);
    assert!(
        shown.contains("global MCP servers: docs (overridden), ⠋ web"),
        "{shown}"
    );
    handle_agent_event(mcp_server_loaded("docs", "docs (3 tools)"), &mut app);
    handle_agent_event(mcp_server_loaded("web", "web (2 tools)"), &mut app);
    let shown = screen(&mut app);
    let global = shown
        .find("loaded global MCP servers: docs (overridden), web (2 tools)")
        .expect(&shown);
    let project = shown
        .find("loaded project MCP servers: docs (3 tools)")
        .expect(&shown);
    assert!(global < project, "{shown}");
    handle_agent_event(mcp_server_restarting("docs", Scope::Project), &mut app);
    let shown = screen(&mut app);
    assert!(shown.contains("project MCP servers: ⠋ docs"), "{shown}");
    assert!(
        shown.contains("loaded global MCP servers: docs (overridden), web (2 tools)"),
        "{shown}"
    );
}

#[test]
fn shows_the_global_line_first_when_every_global_server_is_overridden() {
    let mut app = new_app();
    app.start_mcp_servers([("context7", Scope::Project)], ["context7"]);
    handle_agent_event(
        mcp_server_loaded("context7", "context7 (2 tools)"),
        &mut app,
    );
    let shown = screen(&mut app);
    let global = shown
        .find("loaded global MCP servers: context7 (overridden)")
        .expect(&shown);
    let project = shown
        .find("loaded project MCP servers: context7 (2 tools)")
        .expect(&shown);
    assert!(global < project, "{shown}");
}

#[test]
fn animates_mcp_servers_loading() {
    let mut app = new_app();
    app.start_mcp_servers([("docs", Scope::Project)], []);
    assert!(app.animating());
    app.tick_spinner();
    let shown = screen(&mut app);
    assert!(shown.contains("project MCP servers: ⠙ docs"), "{shown}");
    handle_agent_event(mcp_server_loaded("docs", "docs (3 tools)"), &mut app);
    assert!(!app.animating());
    assert!(screen(&mut app).contains("loaded project MCP servers: docs (3 tools)"));
}

#[test]
fn shows_a_signed_in_server_loading_again_in_its_group() {
    let mut app = new_app();
    app.start_mcp_servers([("figma", Scope::Global)], []);
    handle_agent_event(
        mcp_server_failed("figma", "figma (needs sign-in)", "needs sign-in"),
        &mut app,
    );
    handle_agent_event(mcp_server_restarting("figma", Scope::Global), &mut app);
    let shown = screen(&mut app);
    let group = shown.find("global MCP servers: ⠋ figma").expect(&shown);
    let notice = shown.find("signed in to MCP server figma").expect(&shown);
    assert!(group < notice, "{shown}");
    handle_agent_event(mcp_server_loaded("figma", "figma (5 tools)"), &mut app);
    let shown = screen(&mut app);
    assert!(
        shown.contains("loaded global MCP servers: figma (5 tools)"),
        "{shown}"
    );
    assert_eq!(shown.matches("MCP servers:").count(), 1, "{shown}");
}

#[test]
fn shows_mcp_servers_again_after_clearing_the_session() {
    let mut app = new_app();
    app.start_mcp_servers([("docs", Scope::Project)], []);
    app.clear_session();
    app.push(Role::Event, "new session");
    handle_agent_event(mcp_server_loaded("docs", "docs (3 tools)"), &mut app);
    let texts: Vec<&str> = app
        .messages
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["new session", "loaded project MCP servers: docs (3 tools)"]
    );
}

#[test]
fn shows_only_the_restarted_servers_group_after_clearing_the_session() {
    let mut app = new_app();
    app.start_mcp_servers([("web", Scope::Global), ("docs", Scope::Project)], []);
    handle_agent_event(mcp_server_loaded("web", "web (2 tools)"), &mut app);
    handle_agent_event(mcp_server_loaded("docs", "docs (3 tools)"), &mut app);
    app.clear_session();
    app.push(Role::Event, "new session");
    handle_agent_event(mcp_server_restarting("docs", Scope::Project), &mut app);
    let texts: Vec<&str> = app
        .messages
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "new session",
            "signed in to MCP server docs",
            "project MCP servers: ⠋ docs"
        ]
    );
}
