use std::{collections::HashMap, fmt::Write, sync::Arc, time::Duration};

use serde_json::{Value, json};

mod config_file;
mod connection;
mod headers_helper;
mod http;
mod oauth;
mod stdio;

pub use self::oauth::Pending as SignIn;
use self::{
    config_file::{
        Configs, ServerConfig, enabled, http_url, http_url_of, read_configs, uses_oauth,
    },
    connection::Connection,
};
use crate::{config, skills::Scope, tools};

const PROTOCOL_VERSION: &str = "2025-06-18";
const DEFAULT_TIMEOUT: u64 = 60;
const STDERR_TAIL: usize = 4_000;
const MAX_LINE: usize = 32 << 20;
const MAX_TOOL_NAME: usize = 64;
const NEEDS_SIGN_IN: &str = "needs sign-in";

struct Tool {
    name: String,
    qualified_name: String,
    definition: Value,
}

struct Server {
    name: String,
    scope: Scope,
    connection: Connection,
    tools: Vec<Tool>,
}

fn qualified_name(server: &str, tool: &str) -> String {
    ["mcp__", server, "__", tool]
        .into_iter()
        .flat_map(str::chars)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(MAX_TOOL_NAME)
        .collect()
}

fn tool_definition(qualified_name: &str, mut tool: Value) -> Value {
    let mut schema = match tool["inputSchema"].take() {
        Value::Object(schema) => schema,
        _ => serde_json::Map::new(),
    };
    for key in ["anyOf", "oneOf", "allOf"] {
        schema.remove(key);
    }
    schema.insert("type".into(), json!("object"));
    if !schema.get("properties").is_some_and(Value::is_object) {
        schema.insert("properties".into(), json!({}));
    }
    let description = tool["description"]
        .as_str()
        .or_else(|| tool["title"].as_str())
        .or_else(|| tool["name"].as_str())
        .unwrap_or_default();
    json!({
        "name": qualified_name,
        "description": description,
        "input_schema": schema,
    })
}

async fn list_tools(connection: &Connection) -> Result<Vec<Value>, String> {
    let mut tools = Vec::new();
    let mut cursors = std::collections::HashSet::new();
    let mut params = json!({});
    loop {
        let result = connection.request("tools/list", params).await?;
        tools.extend(result["tools"].as_array().into_iter().flatten().cloned());
        let Some(cursor) = result["nextCursor"].as_str() else {
            return Ok(tools);
        };
        if !cursors.insert(cursor.to_string()) {
            return Err(format!("tools/list repeated the cursor {cursor:?}"));
        }
        params = json!({ "cursor": cursor });
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "Claude Code", "version": env!("CARGO_PKG_VERSION") },
    })
}

async fn connect(name: String, scope: Scope, config: ServerConfig) -> Result<Server, String> {
    let connection = match (config.kind.as_deref(), &config.command, &config.url) {
        (Some("sse"), _, _) => {
            return Err(
                "the SSE transport is not supported; use the server's Streamable HTTP URL".into(),
            );
        }
        (Some("http" | "streamable-http"), _, Some(url)) | (None, None, Some(url)) => {
            Connection::http(&name, &config, url)?
        }
        (Some("http" | "streamable-http"), _, None) => return Err("url is missing".into()),
        (None | Some("stdio"), Some(command), _) => Connection::spawn(&config, command)?,
        (None | Some("stdio"), None, _) => return Err("command is missing".into()),
        (Some(kind), _, _) => return Err(format!("unknown type {kind:?}")),
    };
    let initialize = connection
        .request("initialize", initialize_params())
        .await?;
    connection.notify("notifications/initialized")?;
    let tools = if initialize["capabilities"]["tools"].is_null() {
        Vec::new()
    } else {
        list_tools(&connection)
            .await?
            .into_iter()
            .filter_map(|tool| {
                let tool_name = tool["name"].as_str()?.to_string();
                let qualified_name = qualified_name(&name, &tool_name);
                Some(Tool {
                    name: tool_name,
                    definition: tool_definition(&qualified_name, tool),
                    qualified_name,
                })
            })
            .collect()
    };
    Ok(Server {
        name,
        scope,
        connection,
        tools,
    })
}

fn result_text(result: &Value) -> String {
    let blocks = result["content"].as_array().map_or(&[][..], Vec::as_slice);
    if blocks.is_empty() && !result["structuredContent"].is_null() {
        return result["structuredContent"].to_string();
    }
    let mut text = String::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let mime_type = block["mimeType"].as_str().unwrap_or("unknown type");
        let _ = match block["type"].as_str() {
            Some("text") => text.write_str(block["text"].as_str().unwrap_or_default()),
            Some("image") => write!(text, "[image: {mime_type}]"),
            Some("audio") => write!(text, "[audio: {mime_type}]"),
            Some("resource_link") => write!(
                text,
                "[resource link: {}]",
                block["uri"].as_str().unwrap_or("unknown")
            ),
            Some("resource") => match block["resource"]["text"].as_str() {
                Some(resource_text) => text.write_str(resource_text),
                None => write!(
                    text,
                    "[resource: {}]",
                    block["resource"]["uri"].as_str().unwrap_or("unknown")
                ),
            },
            _ => write!(text, "{block}"),
        };
    }
    text
}

async fn begin_sign_in_with(name: &str, config: &ServerConfig) -> Result<SignIn, String> {
    let url = http_url_of(name, config)?;
    if !uses_oauth(config) {
        return Err(format!(
            "MCP server {name} sends its own Authorization header"
        ));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("cannot set up HTTP: {error}"))?;
    oauth::begin(&client, url, config.oauth.as_ref()).await
}

fn sign_out_with(name: &str, config: &ServerConfig) -> Result<String, String> {
    let url = http_url_of(name, config)?;
    Ok(if oauth::remove(url)? {
        format!("signed out of MCP server {name}")
    } else {
        format!("MCP server {name} was not signed in")
    })
}

/// Forgets the saved sign-in for the configured server `name`.
pub fn sign_out(name: &str) -> Result<String, String> {
    sign_out_with(name, &configured(name)?)
}

/// Starts signing in to the configured server `name`.
pub async fn begin_sign_in(name: &str) -> Result<SignIn, String> {
    begin_sign_in_with(name, &configured(name)?).await
}

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

pub struct Started {
    name: String,
    result: Result<Server, String>,
}

async fn start(name: String, scope: Scope, config: ServerConfig) -> Started {
    let result = connect(name.clone(), scope, config).await;
    Started { name, result }
}

#[cfg(test)]
pub fn failed_start(name: &str, error: &str) -> Started {
    Started {
        name: name.into(),
        result: Err(error.into()),
    }
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

pub struct SignedIn {
    pub user: Option<String>,
}

impl SignInServer {
    /// The sign-in mcp-auth.json holds for this server, if any.
    pub fn signed_in(&self) -> Option<SignedIn> {
        oauth::load(&self.url).map(|saved| SignedIn { user: saved.user })
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
                let tools = server.map_or(0, |server| server.tools.len());
                match server.and_then(|server| server.connection.user()) {
                    Some(user) => format!("{name} ({user}, {tools} tools)"),
                    None => format!("{name} ({tools} tools)"),
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
            "loaded {} MCP server: {name} ({} tools)",
            server.scope,
            server.tools.len()
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
