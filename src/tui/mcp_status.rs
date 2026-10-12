use super::{App, draw::SPINNER_FRAMES, message::Role};
use crate::{mcp, skills::Scope};
use std::borrow::Cow;

pub(super) struct McpGroup {
    scope: Scope,
    servers: Vec<McpEntry>,
    pub(super) message: Option<usize>,
}

impl McpGroup {
    fn loading(&self) -> bool {
        self.servers.iter().any(|server| server.summary.is_none())
    }
}

struct McpEntry {
    name: String,
    summary: Option<String>,
    overridden: bool,
}

fn group_text(spinner_frame: usize, group: &McpGroup) -> String {
    let frame = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
    let entries = group
        .servers
        .iter()
        .map(|server| match &server.summary {
            Some(summary) => Cow::Borrowed(summary.as_str()),
            None => Cow::Owned(format!("{frame} {}", server.name)),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let scope = group.scope;
    if group.loading() {
        format!("{scope} MCP servers: {entries}")
    } else {
        format!("loaded {scope} MCP servers: {entries}")
    }
}

impl App {
    pub(super) fn mcp_loading(&self) -> bool {
        self.mcp_groups.iter().any(McpGroup::loading)
    }

    pub(super) fn show_loading_mcp_groups(&mut self) {
        for index in 0..self.mcp_groups.len() {
            if self.mcp_groups[index].loading() {
                self.show_mcp_group(index);
            }
        }
    }

    /// Shows the servers loading, one line for each scope, global first.
    pub(super) fn start_mcp_servers<'a>(
        &mut self,
        servers: impl IntoIterator<Item = (&'a str, Scope)>,
    ) {
        let mut changed = Vec::new();
        for (name, scope) in servers {
            if let Some((index, server)) = self.mcp_server(name) {
                server.summary = None;
                changed.push(index);
                continue;
            }
            let index = self.mcp_group(scope);
            self.mcp_groups[index].servers.push(McpEntry {
                name: name.into(),
                summary: None,
                overridden: false,
            });
        }
        for index in 0..self.mcp_groups.len() {
            if self.mcp_groups[index].message.is_none() || changed.contains(&index) {
                self.show_mcp_group(index);
            }
        }
    }

    /// Lists global servers that project entries override on the global line.
    pub(super) fn show_overridden_mcp_servers<'a>(
        &mut self,
        names: impl IntoIterator<Item = &'a str>,
    ) {
        let mut names = names.into_iter().peekable();
        if names.peek().is_none() {
            return;
        }
        let index = self.mcp_group(Scope::Global);
        let servers = &mut self.mcp_groups[index].servers;
        servers.extend(names.map(|name| McpEntry {
            name: name.into(),
            summary: Some(format!("{name} (overridden)")),
            overridden: true,
        }));
        servers.sort_by(|first, second| first.name.cmp(&second.name));
        self.show_mcp_group(index);
    }

    fn mcp_group(&mut self, scope: Scope) -> usize {
        if let Some(index) = self
            .mcp_groups
            .iter()
            .position(|group| group.scope == scope)
        {
            return index;
        }
        self.mcp_groups.push(McpGroup {
            scope,
            servers: Vec::new(),
            message: None,
        });
        self.mcp_groups
            .sort_by_key(|group| matches!(group.scope, Scope::Project));
        self.mcp_groups
            .iter()
            .position(|group| group.scope == scope)
            .unwrap_or_default()
    }

    fn mcp_server(&mut self, name: &str) -> Option<(usize, &mut McpEntry)> {
        self.mcp_groups
            .iter_mut()
            .enumerate()
            .find_map(|(index, group)| {
                let server = group
                    .servers
                    .iter_mut()
                    .find(|server| server.name == name && !server.overridden)?;
                Some((index, server))
            })
    }

    pub(super) fn finish_mcp_server(&mut self, added: mcp::Added) {
        match self.mcp_server(&added.name) {
            Some((index, server)) => {
                let needs_sign_in =
                    added.summary.strip_prefix(added.name.as_str()) == Some(" (needs sign-in)");
                server.summary = Some(added.summary);
                self.show_mcp_group(index);
                if added.failed && !needs_sign_in {
                    self.push(Role::Event, added.status);
                }
            }
            None => self.push(Role::Event, added.status),
        }
    }

    fn show_mcp_group(&mut self, index: usize) {
        let text = group_text(self.spinner_frame, &self.mcp_groups[index]);
        let message = self.show_event(self.mcp_groups[index].message, text);
        self.mcp_groups[index].message = Some(message);
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;
    use crate::mcp;
    use crate::tui::{
        Role, UiEvent, handle_agent_event,
        test_support::{new_app, screen},
    };

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

    #[test]
    fn groups_mcp_servers_by_scope_on_two_lines_with_spinners() {
        let mut app = new_app();
        app.start_mcp_servers([
            ("docs", Scope::Project),
            ("figma", Scope::Global),
            ("web", Scope::Global),
        ]);
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
        app.start_mcp_servers([("docs", Scope::Project), ("web", Scope::Global)]);
        app.show_overridden_mcp_servers(["docs"]);
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
        handle_agent_event(
            UiEvent::McpRestarting {
                name: "docs".into(),
                scope: Scope::Project,
                notice: "signed in to MCP server docs".into(),
            },
            &mut app,
        );
        let shown = screen(&mut app);
        assert!(shown.contains("project MCP servers: ⠋ docs"), "{shown}");
        assert!(
            shown.contains("loaded global MCP servers: docs (overridden), web (2 tools)"),
            "{shown}"
        );
    }

    #[test]
    fn animates_mcp_servers_loading() {
        let mut app = new_app();
        app.start_mcp_servers([("docs", Scope::Project)]);
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
        app.start_mcp_servers([("figma", Scope::Global)]);
        handle_agent_event(
            mcp_server_failed("figma", "figma (needs sign-in)", "needs sign-in"),
            &mut app,
        );
        handle_agent_event(
            UiEvent::McpRestarting {
                name: "figma".into(),
                scope: Scope::Global,
                notice: "signed in to MCP server figma".into(),
            },
            &mut app,
        );
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
        app.start_mcp_servers([("docs", Scope::Project)]);
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
}
