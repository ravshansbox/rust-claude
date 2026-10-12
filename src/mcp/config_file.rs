use std::{
    collections::{BTreeMap, HashMap},
    io::ErrorKind,
    path::Path,
};

use serde::Deserialize;
use serde_json::Value;

use crate::skills::{self, Scope};

#[derive(Default, Deserialize)]
pub(super) struct ServerConfig {
    #[serde(rename = "type")]
    pub(super) kind: Option<String>,
    pub(super) command: Option<String>,
    #[serde(default)]
    pub(super) args: Vec<String>,
    #[serde(default)]
    pub(super) env: HashMap<String, String>,
    pub(super) cwd: Option<String>,
    pub(super) url: Option<String>,
    #[serde(default)]
    pub(super) headers: HashMap<String, String>,
    #[serde(rename = "headersHelper")]
    pub(super) headers_helper: Option<String>,
    pub(super) oauth: Option<OAuthConfig>,
    pub(super) enabled: Option<bool>,
    pub(super) timeout: Option<u64>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OAuthConfig {
    pub(super) client_id: Option<String>,
    pub(super) client_secret: Option<String>,
}

/// HTTP servers sign in with OAuth unless their entry sets an
/// `Authorization` header or a `headersHelper`.
pub(super) fn uses_oauth(config: &ServerConfig) -> bool {
    config.headers_helper.is_none()
        && !config
            .headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("authorization"))
}

pub(super) fn enabled(config: &ServerConfig) -> bool {
    config.enabled != Some(false)
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

pub(super) fn expand_home(text: &str, home: Option<&Path>) -> String {
    match (text.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => text.to_string(),
    }
}

/// Replaces each `${NAME}` with the environment variable `NAME`.
pub(super) fn expand_variables(text: &str) -> Result<String, String> {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            rest = &rest[start..];
            break;
        };
        let name = &after[..end];
        expanded.push_str(&std::env::var(name).map_err(|_| name.to_string())?);
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
}

pub(super) type Configs = BTreeMap<String, (Scope, ServerConfig)>;

pub(super) fn read_config(
    path: &Path,
    scope: Scope,
    servers: &mut Configs,
    diagnostics: &mut Vec<String>,
) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => {
            diagnostics.push(format!("MCP: cannot read {}: {error}", path.display()));
            return;
        }
    };
    let config: Value = match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(error) => {
            diagnostics.push(format!("MCP: invalid {}: {error}", path.display()));
            return;
        }
    };
    let Some(entries) = config["mcpServers"].as_object() else {
        if !config["mcpServers"].is_null() {
            diagnostics.push(format!(
                "MCP: mcpServers in {} must be an object",
                path.display()
            ));
        }
        return;
    };
    for (name, entry) in entries {
        if !valid_server_name(name) {
            diagnostics.push(format!(
                "MCP: server name {name:?} may only contain letters, digits, _ and -"
            ));
            continue;
        }
        match ServerConfig::deserialize(entry) {
            Ok(server) if server.timeout == Some(0) => {
                servers.remove(name);
                diagnostics.push(format!("MCP server {name}: timeout must be at least 1"));
            }
            Ok(server) => {
                servers.insert(name.clone(), (scope, server));
            }
            Err(error) => {
                servers.remove(name);
                diagnostics.push(format!("MCP server {name}: invalid entry: {error}"));
            }
        }
    }
}

/// Reads the global and project configs. Also gives the enabled global
/// servers that project entries override.
pub(super) fn read_configs(
    config_dir: Option<&Path>,
    cwd: &Path,
) -> (Configs, Vec<String>, Vec<String>) {
    let mut configs = BTreeMap::new();
    let mut diagnostics = Vec::new();
    if let Some(config_dir) = config_dir {
        read_config(
            &config_dir.join("mcp.json"),
            Scope::Global,
            &mut configs,
            &mut diagnostics,
        );
    }
    let global: Vec<String> = configs
        .iter()
        .filter(|(_, (_, config))| enabled(config))
        .map(|(name, _)| name.clone())
        .collect();
    let project_dir = cwd.join(".rust-coding-agent");
    if !config_dir.is_some_and(|config_dir| skills::same_dir(config_dir, &project_dir)) {
        read_config(
            &project_dir.join("mcp.json"),
            Scope::Project,
            &mut configs,
            &mut diagnostics,
        );
    }
    let overridden = global
        .into_iter()
        .filter(|name| {
            configs
                .get(name)
                .is_some_and(|(scope, _)| *scope == Scope::Project)
        })
        .collect();
    (configs, overridden, diagnostics)
}

/// The URL of an HTTP server entry, or `None` for a stdio one.
pub(super) fn http_url(config: &ServerConfig) -> Option<&str> {
    match (config.kind.as_deref(), &config.command) {
        (Some("http" | "streamable-http"), _) | (None, None) => config.url.as_deref(),
        _ => None,
    }
}

pub(super) fn http_url_of<'a>(name: &str, config: &'a ServerConfig) -> Result<&'a str, String> {
    http_url(config).ok_or_else(|| format!("MCP server {name} is not an HTTP server"))
}
