use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tokens are renewed this long before they expire.
const EXPIRY_MARGIN: u128 = 60_000;

/// Serialises changes to mcp-auth.json within this process.
static FILE: Mutex<()> = Mutex::new(());

/// The sign-in for one server URL, as saved in mcp-auth.json.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Saved {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the access token expires, in milliseconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u128>,
}

impl Saved {
    pub fn expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| now_millis() + EXPIRY_MARGIN >= expires_at)
    }
}

pub(super) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn path() -> Option<PathBuf> {
    Some(crate::config::dir()?.join("mcp-auth.json"))
}

fn read_all() -> BTreeMap<String, Saved> {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_all(saved: &BTreeMap<String, Saved>) -> Result<(), String> {
    let path = path().ok_or("HOME is not set")?;
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            crate::config::create_private_dir(parent)?;
        }
        let text = serde_json::to_string_pretty(saved)?;
        crate::config::write_private_file(&path, text.as_bytes())
    };
    write().map_err(|error| format!("cannot save {}: {error}", path.display()))
}

pub(super) fn load(server_url: &str) -> Option<Saved> {
    let _file = FILE.lock();
    read_all().remove(server_url)
}

pub(super) fn save(server_url: &str, saved: &Saved) -> Result<(), String> {
    let _file = FILE.lock();
    let mut all = read_all();
    all.insert(server_url.to_string(), saved.clone());
    write_all(&all)
}

/// Asks the token endpoint for tokens with `form`, and adds the client's
/// credentials. Fields the reply leaves out are kept from `saved`.
pub(super) async fn request_tokens(
    client: &reqwest::Client,
    saved: &Saved,
    form: &[(&str, &str)],
) -> Result<Saved, String> {
    let mut form: Vec<(&str, &str)> = form.to_vec();
    form.push(("client_id", &saved.client_id));
    if let Some(secret) = &saved.client_secret {
        form.push(("client_secret", secret));
    }
    if let Some(resource) = &saved.resource {
        form.push(("resource", resource));
    }
    let response = client
        .post(&saved.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("cannot reach {}: {error}", saved.token_endpoint))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let reply: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let Some(access_token) = reply["access_token"]
        .as_str()
        .filter(|_| status.is_success())
    else {
        let reason = reply["error_description"]
            .as_str()
            .or(reply["error"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| body.trim().chars().take(200).collect());
        return Err(format!(
            "token endpoint answered with status {}: {reason}",
            status.as_u16()
        ));
    };
    Ok(Saved {
        access_token: access_token.to_string(),
        refresh_token: reply["refresh_token"]
            .as_str()
            .map(str::to_string)
            .or_else(|| saved.refresh_token.clone()),
        expires_at: reply["expires_in"]
            .as_u64()
            .map(|seconds| now_millis() + u128::from(seconds) * 1000),
        ..saved.clone()
    })
}

/// Renews `saved` with its refresh token and saves the new tokens. Tokens
/// another copy of rust-claude saved meanwhile are used instead, unless they
/// are the ones the server turned down.
pub(super) async fn refresh(
    client: &reqwest::Client,
    server_url: &str,
    saved: &Saved,
    rejected: Option<&str>,
) -> Result<Saved, String> {
    if let Some(current) = load(server_url)
        && current.access_token != saved.access_token
        && Some(current.access_token.as_str()) != rejected
        && !current.expired()
    {
        return Ok(current);
    }
    let refresh_token = saved.refresh_token.as_deref().ok_or("no refresh token")?;
    let renewed = request_tokens(
        client,
        saved,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    save(server_url, &renewed)?;
    Ok(renewed)
}
