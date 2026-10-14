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

    /// Shows the servers loading, one line for each scope, global first, with
    /// global servers that project entries override on the global line.
    pub(super) fn start_mcp_servers<'a>(
        &mut self,
        servers: impl IntoIterator<Item = (&'a str, Scope)>,
        overridden: impl IntoIterator<Item = &'a str>,
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
        let mut overridden = overridden.into_iter().peekable();
        if overridden.peek().is_some() {
            let index = self.mcp_group(Scope::Global);
            let servers = &mut self.mcp_groups[index].servers;
            servers.extend(overridden.map(|name| McpEntry {
                name: name.into(),
                summary: Some(format!("{name} (overridden)")),
                overridden: true,
            }));
            servers.sort_by(|first, second| first.name.cmp(&second.name));
        }
        for index in 0..self.mcp_groups.len() {
            if self.mcp_groups[index].message.is_none() || changed.contains(&index) {
                self.show_mcp_group(index);
            }
        }
    }

    fn mcp_group(&mut self, scope: Scope) -> usize {
        let position = |groups: &[McpGroup]| groups.iter().position(|group| group.scope == scope);
        if let Some(index) = position(&self.mcp_groups) {
            return index;
        }
        self.mcp_groups.push(McpGroup {
            scope,
            servers: Vec::new(),
            message: None,
        });
        self.mcp_groups
            .sort_by_key(|group| matches!(group.scope, Scope::Project));
        position(&self.mcp_groups).unwrap_or_default()
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
mod tests;
