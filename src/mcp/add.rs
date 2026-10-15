use std::{collections::HashMap, sync::Arc};

use super::{Mcp, NEEDS_SIGN_IN, Started};

pub struct Added {
    pub name: String,
    pub status: String,
    /// The server's name with its tool count or why it did not load.
    pub summary: String,
    pub failed: bool,
    pub diagnostics: Vec<String>,
}

fn sign_in_summary(name: &str, failed: &str) -> Option<String> {
    failed
        .strip_prefix("MCP server ")
        .and_then(|rest| rest.strip_prefix(name))
        .and_then(|rest| rest.strip_prefix(' '))
        .is_some_and(|rest| rest.starts_with(NEEDS_SIGN_IN))
        .then(|| format!("{name} ({NEEDS_SIGN_IN})"))
}

impl Mcp {
    pub(super) fn from_started(
        diagnostics: Vec<String>,
        started: impl IntoIterator<Item = Started>,
    ) -> Self {
        let mut mcp = Self {
            servers: Vec::new(),
            diagnostics,
        };
        for started in started {
            let name = started.name.clone();
            let (diagnostics, status) = mcp.add_server(started);
            mcp.diagnostics.extend(diagnostics);
            if let Err(failed) = status {
                mcp.diagnostics
                    .push(sign_in_summary(&name, &failed).unwrap_or(failed));
            }
        }
        mcp
    }

    pub fn add(&mut self, started: Started) -> Added {
        let name = started.name.clone();
        let (diagnostics, status) = self.add_server(started);
        let summary = match &status {
            Ok(_) => {
                let server = self.servers.last();
                let tools =
                    crate::tools::plural(server.map_or(0, |server| server.tools.len()), "tool");
                match server.and_then(|server| server.connection.user()) {
                    Some(user) => format!("{name} ({user}, {tools})"),
                    None => format!("{name} ({tools})"),
                }
            }
            Err(failed) => {
                sign_in_summary(&name, failed).unwrap_or_else(|| format!("{name} (failed)"))
            }
        };
        Added {
            name,
            failed: status.is_err(),
            status: status.unwrap_or_else(|failed| failed),
            summary,
            diagnostics,
        }
    }

    /// Adds a server that started again in place of the old one.
    pub fn replace(&mut self, started: Started) -> Added {
        self.servers.retain(|server| server.name != started.name);
        self.add(started)
    }

    fn add_server(&mut self, started: Started) -> (Vec<String>, Result<String, String>) {
        let Started { name, result } = started;
        let mut server = match result {
            Ok(server) => server,
            Err(error) => {
                let failed = if error.starts_with(NEEDS_SIGN_IN) {
                    format!("MCP server {name} {error}")
                } else {
                    format!("MCP server {name} failed: {error}")
                };
                return (Vec::new(), Err(failed));
            }
        };
        let mut diagnostics = Vec::new();
        let mut seen: HashMap<String, String> = self
            .servers
            .iter()
            .flat_map(|server| {
                server.tools.iter().map(|tool| {
                    (
                        tool.qualified_name.clone(),
                        format!("{}/{}", server.name, tool.name),
                    )
                })
            })
            .collect();
        server.tools.retain(|tool| {
            if let Some(other) = seen.get(&tool.qualified_name) {
                diagnostics.push(format!(
                    "MCP server {name}: skipped tool {} because its name clashes with {other}",
                    tool.name
                ));
                return false;
            }
            seen.insert(tool.qualified_name.clone(), format!("{name}/{}", tool.name));
            true
        });
        let loaded = format!(
            "loaded {} MCP server: {name} ({})",
            server.scope,
            crate::tools::plural(server.tools.len(), "tool")
        );
        self.servers.push(Arc::new(server));
        (diagnostics, Ok(loaded))
    }
}
