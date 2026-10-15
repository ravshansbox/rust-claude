use std::sync::Arc;

use serde_json::{Value, json};

mod add;
mod config_file;
mod connection;
mod headers_helper;
mod http;
mod oauth;
mod server;
mod sign_in;
mod startup;
mod stdio;
mod tool;

pub use self::add::Added;
pub use self::oauth::Pending as SignIn;
pub use self::server::Started;
#[cfg(test)]
pub use self::server::failed_start;
pub use self::sign_in::{begin_sign_in, sign_out};
pub use self::startup::{SignInServer, StartingServer, Startup, restart, startup};
use self::{
    config_file::{Configs, ServerConfig, read_configs},
    server::{Server, initialize_params},
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

impl Mcp {
    pub async fn load() -> Self {
        let startup = startup();
        let servers = startup.servers.into_iter().map(|pending| pending.started);
        let started = futures::future::join_all(servers).await;
        Self::from_started(startup.diagnostics, started)
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
