use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use serde::Deserialize;

use crate::skills::Scope;

mod read;

#[cfg(test)]
pub(super) use read::read_config;
pub(super) use read::read_configs;

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
