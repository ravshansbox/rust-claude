use std::{collections::BTreeMap, fs::File, io, path::PathBuf, sync::Mutex};

use crate::{
    auth::now_millis,
    config,
    mcp::config_file::{OAuthConfig, expand_variables},
};
use serde::{Deserialize, Serialize};

/// Serialises changes to mcp-auth.json within this process.
static FILE: Mutex<()> = Mutex::new(());

/// The sign-in for one server URL, as saved in mcp-auth.json.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(in crate::mcp) struct Saved {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the access token is renewed, in milliseconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u128>,
    /// Who signed in, as named in the tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Whether `client_secret` comes from mcp.json, so it is not saved.
    #[serde(skip)]
    pub secret_from_config: bool,
}

impl Saved {
    pub fn expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| now_millis() >= expires_at)
    }
}

fn path() -> Option<PathBuf> {
    Some(config::dir()?.join("mcp-auth.json"))
}

/// The lock file `name` next to mcp-auth.json, with its folder created.
fn lock_path(name: &str) -> Option<PathBuf> {
    let path = path()?.with_file_name(name);
    config::create_private_dir(path.parent()?).ok()?;
    Some(path)
}

/// Locks mcp-auth.lock until the returned file is closed, so running copies
/// change mcp-auth.json one at a time instead of undoing each other's
/// changes. Changes are quick, so this blocks while it waits.
fn lock_changes() -> Option<File> {
    config::lock_file_blocking(&lock_path("mcp-auth.lock")?).ok()
}

/// Locks mcp-renew.lock until the returned file is closed, so running
/// copies renew sign-ins one at a time.
pub(in crate::mcp) async fn lock_renewals() -> Option<File> {
    config::lock_file(&lock_path("mcp-renew.lock")?).await.ok()
}

fn read_all() -> BTreeMap<String, Saved> {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_all(saved: &BTreeMap<String, Saved>) -> Result<(), String> {
    let path = path().ok_or("HOME is not set")?;
    let write = || -> io::Result<()> {
        let text = serde_json::to_string_pretty(saved)?;
        config::write_private_file(&path, text.as_bytes())
    };
    write().map_err(|error| format!("cannot save {}: {error}", path.display()))
}

pub(in crate::mcp) fn load(server_url: &str) -> Option<Saved> {
    let _file = FILE.lock();
    read_all().remove(server_url)
}

/// Loads the sign-in for `server_url`, with the client secret from
/// mcp.json when it is for the client `oauth` names.
pub(in crate::mcp) fn load_for(server_url: &str, oauth: Option<&OAuthConfig>) -> Option<Saved> {
    let mut saved = load(server_url)?;
    if let Some(oauth) = oauth
        && oauth.client_id.as_deref() == Some(saved.client_id.as_str())
        && let Some(secret) = &oauth.client_secret
    {
        saved.client_secret = expand_variables(secret).ok();
        saved.secret_from_config = true;
    }
    Some(saved)
}

pub(in crate::mcp) fn save(server_url: &str, saved: &Saved) -> Result<(), String> {
    let _file = FILE.lock();
    // Without the lock, saving still works; copies just may not take turns.
    let _lock = lock_changes();
    let mut all = read_all();
    let mut saved = saved.clone();
    if saved.secret_from_config {
        saved.client_secret = None;
    }
    all.insert(server_url.to_string(), saved);
    write_all(&all)
}

/// Forgets the sign-in for `server_url`. Returns whether there was one.
pub(in crate::mcp) fn remove(server_url: &str) -> Result<bool, String> {
    let _file = FILE.lock();
    let _lock = lock_changes();
    let mut all = read_all();
    if all.remove(server_url).is_none() {
        return Ok(false);
    }
    write_all(&all)?;
    Ok(true)
}
