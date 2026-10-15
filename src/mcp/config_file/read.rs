use std::{collections::BTreeMap, io::ErrorKind, path::Path};

use serde::Deserialize;
use serde_json::Value;

use super::{Configs, ServerConfig, enabled};
use crate::skills::{self, Scope};

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

pub(in crate::mcp) fn read_config(
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
pub(in crate::mcp) fn read_configs(
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
