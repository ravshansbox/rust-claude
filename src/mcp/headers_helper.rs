use std::{collections::HashMap, process::Stdio, time::Duration};

use tokio::io::{AsyncRead, AsyncReadExt};

use super::http::MAX_ERROR_BODY;
use crate::tools::{self, ProcessGroup};

const HEADERS_HELPER_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct HeadersHelper {
    pub(super) command: String,
    pub(super) server_name: String,
}

impl HeadersHelper {
    /// Runs the command and reads the JSON object of headers it prints.
    pub(super) async fn run(&self, url: &str) -> Result<reqwest::header::HeaderMap, String> {
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(&self.command)
            .env("CLAUDE_CODE_MCP_SERVER_NAME", &self.server_name)
            .env("CLAUDE_CODE_MCP_SERVER_URL", url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        tools::new_session(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot run headersHelper: {error}"))?;
        // Stops everything the command started when it exits, times out or
        // is no longer waited for.
        let group = ProcessGroup(child.id());
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err("cannot read headersHelper output".into());
        };
        let stdout = tokio::spawn(read_all(stdout));
        let stderr = tokio::spawn(read_all(stderr));
        let status = tokio::time::timeout(HEADERS_HELPER_TIMEOUT, child.wait()).await;
        // Processes it left running would keep its output open.
        drop(group);
        let status = status
            .map_err(|_| {
                format!(
                    "headersHelper timed out after {} seconds",
                    HEADERS_HELPER_TIMEOUT.as_secs()
                )
            })?
            .map_err(|error| format!("cannot run headersHelper: {error}"))?;
        let stdout = stdout.await.unwrap_or_default();
        let stderr = stderr.await.unwrap_or_default();
        if !status.success() {
            let status = status
                .code()
                .map_or_else(|| "a signal".to_string(), |code| format!("status {code}"));
            let stderr = String::from_utf8_lossy(&stderr);
            let stderr: String = stderr.trim().chars().take(MAX_ERROR_BODY).collect();
            return Err(if stderr.is_empty() {
                format!("headersHelper exited with {status}")
            } else {
                format!("headersHelper exited with {status}: {stderr}")
            });
        }
        let headers: HashMap<String, String> = serde_json::from_slice(&stdout)
            .map_err(|_| "headersHelper must print a JSON object of strings".to_string())?;
        header_map(&headers, "headersHelper gave ", |_, value| {
            Ok(value.to_string())
        })
    }
}

pub(super) fn header_map(
    headers: &HashMap<String, String>,
    prefix: &str,
    value: impl Fn(&str, &str) -> Result<String, String>,
) -> Result<reqwest::header::HeaderMap, String> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, text) in headers {
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("{prefix}invalid header name {name:?}"))?;
        let value = reqwest::header::HeaderValue::from_str(&value(name, text)?)
            .map_err(|_| format!("{prefix}invalid value for header {name}"))?;
        map.insert(header_name, value);
    }
    Ok(map)
}

async fn read_all(mut reader: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output).await;
    output
}
