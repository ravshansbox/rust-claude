use std::{
    fmt,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config;

mod login;
mod renew;
mod tokens;

use login::login;
pub use renew::finish_renewal;
use tokens::request_tokens;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    access: String,
    refresh: String,
    expires: u128,
    /// What to tell the user about the last renewal, until it is shown.
    #[serde(skip)]
    renewal_notice: Option<String>,
}

fn credentials_path() -> Result<PathBuf> {
    config::file("auth.json")
}

pub(crate) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// The saved sign-in cannot be used any more, so only signing in again helps:
/// auth.json is not valid, or the token endpoint turned the request down.
#[derive(Debug)]
struct Unusable(String);

impl fmt::Display for Unusable {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Unusable {}

impl Credentials {
    /// Signs in again only when the saved sign-in cannot be used. Other
    /// errors, such as no network or a server error while renewing, are
    /// returned, so the saved sign-in is kept for the next try.
    pub async fn load_or_login(http: &reqwest::Client) -> Result<Self> {
        let Ok(text) = std::fs::read_to_string(credentials_path()?) else {
            return login(http).await;
        };
        match Self::load_and_refresh(&text, http).await {
            Ok(credentials) => Ok(credentials),
            Err(error) if error.is::<Unusable>() => {
                eprintln!("Saved sign-in is not usable ({error}). Sign in again.\n");
                login(http).await
            }
            Err(error) => Err(error),
        }
    }

    async fn load_and_refresh(text: &str, http: &reqwest::Client) -> Result<Self> {
        let mut credentials: Self =
            serde_json::from_str(text).map_err(|error| Unusable(error.to_string()))?;
        if now_millis() >= credentials.expires {
            credentials.renew(http, None).await?;
        }
        Ok(credentials)
    }

    pub fn take_renewal_notice(&mut self) -> Option<String> {
        self.renewal_notice.take()
    }

    /// Saves auth.json atomically, so readers never see a half-written file
    /// and a crash keeps the old sign-in.
    fn save(&self) -> Result<()> {
        let path = credentials_path()?;
        config::write_private_file(&path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

pub(crate) fn random_token() -> Result<String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) fn expires_at(seconds: Option<u64>, margin: u128) -> Option<u128> {
    let lifetime = u128::from(seconds?) * 1000;
    Some(now_millis() + lifetime - lifetime.min(2 * margin) / 2)
}

#[cfg(test)]
pub(crate) mod tests;
