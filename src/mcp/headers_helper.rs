use std::{collections::HashMap, process::Stdio, time::Duration};

use serde_json::Value;

use super::http::MAX_ERROR_BODY;

const HEADERS_HELPER_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct HeadersHelper {
    pub(super) command: String,
    pub(super) server_name: String,
}

impl HeadersHelper {
    /// Runs the command and reads the JSON object of headers it prints.
    pub(super) async fn run(&self, url: &str) -> Result<reqwest::header::HeaderMap, String> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .env("CLAUDE_CODE_MCP_SERVER_NAME", &self.server_name)
            .env("CLAUDE_CODE_MCP_SERVER_URL", url)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(HEADERS_HELPER_TIMEOUT, output)
            .await
            .map_err(|_| {
                format!(
                    "headersHelper timed out after {} seconds",
                    HEADERS_HELPER_TIMEOUT.as_secs()
                )
            })?
            .map_err(|error| format!("cannot run headersHelper: {error}"))?;
        if !output.status.success() {
            let status = output
                .status
                .code()
                .map_or_else(|| "a signal".to_string(), |code| format!("status {code}"));
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr: String = stderr.trim().chars().take(MAX_ERROR_BODY).collect();
            return Err(if stderr.is_empty() {
                format!("headersHelper exited with {status}")
            } else {
                format!("headersHelper exited with {status}: {stderr}")
            });
        }
        let invalid = || "headersHelper must print a JSON object of strings".to_string();
        let headers: HashMap<String, Value> =
            serde_json::from_slice(&output.stdout).map_err(|_| invalid())?;
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            let value = value.as_str().ok_or_else(invalid)?;
            let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| format!("headersHelper gave invalid header name {name:?}"))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| format!("headersHelper gave invalid value for header {name}"))?;
            map.insert(header_name, value);
        }
        Ok(map)
    }
}
