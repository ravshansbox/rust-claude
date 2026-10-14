use std::{collections::HashMap, sync::Arc};

use serde_json::{Value, json};

mod config_file;
mod connection;
mod headers_helper;
mod http;
mod oauth;
mod server;
mod sign_in;
mod stdio;
mod tool;

pub use self::oauth::Pending as SignIn;
pub use self::server::Started;
#[cfg(test)]
pub use self::server::failed_start;
pub use self::sign_in::{begin_sign_in, sign_out};
use self::{
    config_file::{Configs, ServerConfig, enabled, http_url, read_configs, uses_oauth},
    server::{Server, initialize_params, start},
    tool::result_text,
};
use crate::{config, skills::Scope, tools};

const DEFAULT_TIMEOUT: u64 = 60;
const STDERR_TAIL: usize = 4_000;
const MAX_LINE: usize = 32 << 20;
const NEEDS_SIGN_IN: &str = "needs sign-in";

fn configs() -> (Configs, Vec<String>, Vec<String>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    read_configs(config::dir().as_deref(), &cwd)
}

fn configured(name: &str) -> Result<ServerConfig, String> {
    configs()
        .0
        .remove(name)
        .map(|(_, config)| config)
        .ok_or_else(|| format!("no MCP server named {name}"))
}

/// Cloning shares the running servers, so subagents can call their tools.
#[derive(Clone, Default)]
pub struct Mcp {
    servers: Vec<Arc<Server>>,
    pub diagnostics: Vec<String>,
}

pub struct StartingServer {
    pub name: String,
    pub scope: Scope,
    pub started: futures::future::BoxFuture<'static, Started>,
}

pub struct Added {
    pub name: String,
    pub status: String,
    /// The server's name with its tool count or why it did not load.
    pub summary: String,
    pub failed: bool,
    pub diagnostics: Vec<String>,
}

pub struct Startup {
    pub diagnostics: Vec<String>,
    pub servers: Vec<StartingServer>,
    /// Global servers that project entries override.
    pub overridden: Vec<String>,
    /// HTTP servers that sign in with OAuth.
    pub sign_in_servers: Vec<SignInServer>,
}

#[derive(Clone)]
pub struct SignInServer {
    pub name: String,
    pub url: String,
}

impl SignInServer {
    /// Whether mcp-auth.json holds a sign-in for this server.
    pub fn signed_in(&self) -> bool {
        oauth::load(&self.url).is_some()
    }
}

fn starting(name: String, scope: Scope, config: ServerConfig) -> StartingServer {
    StartingServer {
        scope,
        started: Box::pin(start(name.clone(), scope, config)),
        name,
    }
}

/// Starts the configured server `name` again, as after signing in.
pub fn restart(name: &str) -> Option<StartingServer> {
    restart_from(configs().0, name)
}

fn restart_from(mut configs: Configs, name: &str) -> Option<StartingServer> {
    let (scope, config) = configs.remove(name).filter(|(_, config)| enabled(config))?;
    Some(starting(name.to_string(), scope, config))
}

pub fn startup() -> Startup {
    let (configs, overridden, diagnostics) = configs();
    let enabled: Vec<_> = configs
        .into_iter()
        .filter(|(_, (_, config))| enabled(config))
        .collect();
    let sign_in_servers = enabled
        .iter()
        .filter(|(_, (_, config))| uses_oauth(config))
        .filter_map(|(name, (_, config))| {
            Some(SignInServer {
                name: name.clone(),
                url: http_url(config)?.to_string(),
            })
        })
        .collect();
    let servers = enabled
        .into_iter()
        .map(|(name, (scope, config))| starting(name, scope, config))
        .collect();
    Startup {
        diagnostics,
        servers,
        overridden,
        sign_in_servers,
    }
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
    pub async fn load() -> Self {
        let startup = startup();
        let servers = startup.servers.into_iter().map(|pending| pending.started);
        let started = futures::future::join_all(servers).await;
        Self::from_started(startup.diagnostics, started)
    }

    fn from_started(diagnostics: Vec<String>, started: impl IntoIterator<Item = Started>) -> Self {
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

    pub fn definitions(&self) -> impl Iterator<Item = Value> + '_ {
        let mut servers: Vec<_> = self.servers.iter().collect();
        servers.sort_by(|first, second| {
            (first.scope == Scope::Project, &first.name)
                .cmp(&(second.scope == Scope::Project, &second.name))
        });
        servers
            .into_iter()
            .flat_map(|server| &server.tools)
            .map(|tool| tool.definition.clone())
    }

    pub async fn call(&self, name: &str, input: &Value) -> Option<Result<String, String>> {
        let (server, tool) = self.servers.iter().find_map(|server| {
            server
                .tools
                .iter()
                .find(|tool| tool.qualified_name == name)
                .map(|tool| (server, tool))
        })?;
        let arguments = if input.is_object() {
            input.clone()
        } else {
            json!({})
        };
        let result = server
            .connection
            .request(
                "tools/call",
                json!({ "name": tool.name, "arguments": arguments }),
            )
            .await
            .and_then(|result| {
                let text = tools::truncate(result_text(&result));
                if result["isError"] == true {
                    Err(text)
                } else {
                    Ok(text)
                }
            });
        Some(result)
    }

    /// What to tell the user about sign-ins renewed since the last call.
    pub fn take_renewal_notices(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter_map(|server| server.connection.take_renewal_notice())
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests;
