use std::{
    fmt,
    fs::File,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{agent::retry::retryable, config};

mod login;
mod tokens;

use login::login;
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

/// Locks auth.lock until the returned file is closed, so running copies
/// renew the sign-in one at a time.
async fn lock_credentials() -> Result<File> {
    let path = credentials_path()?.with_file_name("auth.lock");
    config::create_private_dir(path.parent().context("invalid credentials path")?)?;
    Ok(config::lock_file(&path).await?)
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

/// Held by each renewal until it has saved what it got, so quitting can wait
/// for it.
static RENEWING: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Waits up to `limit` for running renewals to finish. Exiting the process
/// during one could lose new tokens after the endpoint has spent ours.
pub async fn finish_renewal(limit: Duration) {
    let _ = tokio::time::timeout(limit, RENEWING.write()).await;
}

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

    /// Renews the tokens in their own task, so a cancelled prompt or start-up
    /// request cannot drop the new tokens after the endpoint has spent ours.
    /// Running copies take turns, and each first uses what the one before it
    /// saved: its new refresh token replaces ours. Tokens count as usable
    /// until they expire or until the API has turned down `rejected`, the
    /// access token they hold.
    async fn renew(&mut self, http: &reqwest::Client, rejected: Option<&str>) -> Result<()> {
        let http = http.clone();
        let rejected = rejected.map(str::to_string);
        let mut credentials = self.clone();
        *self = tokio::spawn(async move {
            let usable = |credentials: &Self| {
                now_millis() < credentials.expires
                    && rejected.as_deref() != Some(credentials.access.as_str())
            };
            let _renewing = RENEWING.read().await;
            // Without the lock, renewing still works; copies just may not
            // take turns.
            let _lock = lock_credentials().await.ok();
            credentials.adopt_saved();
            if usable(&credentials) {
                return Ok(credentials);
            }
            match credentials.refresh(&http).await {
                Ok(renewed) => Ok(renewed),
                // A rust-coding-agent that does not take turns may have renewed
                // the tokens meanwhile, spending the refresh token we hold.
                Err(error) => {
                    credentials.adopt_saved();
                    if usable(&credentials) {
                        return Ok(credentials);
                    }
                    // Turned-down tokens that cannot be renewed would still
                    // look usable to the next start until they expire, so
                    // mark them expired: it then renews them or signs in.
                    if rejected.is_some() && error.is::<Unusable>() {
                        credentials.expires = 0;
                        let _ = credentials.save();
                    }
                    Err(error)
                }
            }
        })
        .await??;
        Ok(())
    }

    async fn refresh(&self, http: &reqwest::Client) -> Result<Self> {
        let mut credentials = request_tokens(
            http,
            json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": self.refresh,
            }),
            Some(&self.refresh),
        )
        .await?;
        // The endpoint has spent our refresh token, so keep the new pair even
        // when auth.json cannot take it.
        credentials.renewal_notice = Some(match credentials.save() {
            Ok(()) => "renewed sign-in token".into(),
            Err(error) => {
                format!("renewed sign-in token, but could not save it to auth.json: {error:#}")
            }
        });
        Ok(credentials)
    }

    /// Renews the tokens when they have expired. Failures other than the
    /// endpoint turning the sign-in down, such as no network or a server
    /// error, may pass, so requests retry them.
    pub async fn access_token(&mut self, http: &reqwest::Client) -> Result<String> {
        if now_millis() < self.expires {
            return Ok(self.access.clone());
        }
        let renewed = self.renew(http, None).await;
        self.renewed_token(renewed, "sign-in expired")
    }

    /// Renews the tokens after the API turned down `rejected`, the access
    /// token sent, unless another renewal has replaced it since. Failures
    /// are reported like those of `access_token`.
    pub async fn renew_rejected(
        &mut self,
        http: &reqwest::Client,
        rejected: &str,
    ) -> Result<String> {
        let renewed = self.renew(http, Some(rejected)).await;
        self.renewed_token(renewed, "sign-in turned down")
    }

    fn renewed_token(&self, renewed: Result<()>, problem: &str) -> Result<String> {
        match renewed {
            Ok(()) => Ok(self.access.clone()),
            Err(error) if error.is::<Unusable>() => Err(error.context(format!(
                "{problem}: restart rust-coding-agent to sign in again"
            ))),
            Err(error) => Err(retryable(
                format!("could not renew the sign-in: {error:#}"),
                None,
            )),
        }
    }

    /// Switches to the tokens in auth.json when they last longer than ours,
    /// or as long with another refresh token, as tokens without expiry do.
    fn adopt_saved(&mut self) {
        let saved = credentials_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Self>(&text).ok());
        if let Some(saved) = saved
            && (saved.expires > self.expires
                || (saved.expires == self.expires && saved.refresh != self.refresh))
        {
            *self = saved;
        }
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
