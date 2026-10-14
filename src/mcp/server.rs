use serde_json::{Value, json};

use super::{
    config_file::ServerConfig,
    connection::Connection,
    tool::{Tool, qualified_name, tool_definition},
};
use crate::skills::Scope;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub(super) struct Server {
    pub(super) name: String,
    pub(super) scope: Scope,
    pub(super) connection: Connection,
    pub(super) tools: Vec<Tool>,
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

pub(super) fn initialize_params() -> Value {
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

pub struct Started {
    pub(super) name: String,
    pub(super) result: Result<Server, String>,
}

pub(super) async fn start(name: String, scope: Scope, config: ServerConfig) -> Started {
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
