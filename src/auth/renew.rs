use std::{fs::File, time::Duration};

use anyhow::{Context, Result};
use serde_json::json;

use super::{
    CLIENT_ID, Credentials, Unusable, credentials_path, now_millis, tokens::request_tokens,
};
use crate::{agent::retry::retryable, config};

/// Locks auth.lock until the returned file is closed, so running copies
/// renew the sign-in one at a time.
async fn lock_credentials() -> Result<File> {
    let path = credentials_path()?.with_file_name("auth.lock");
    config::create_private_dir(path.parent().context("invalid credentials path")?)?;
    Ok(config::lock_file(&path).await?)
}

/// Held by each renewal until it has saved what it got, so quitting can wait
/// for it.
static RENEWING: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Waits up to `limit` for running renewals to finish. Exiting the process
/// during one could lose new tokens after the endpoint has spent ours.
pub async fn finish_renewal(limit: Duration) {
    let _ = tokio::time::timeout(limit, RENEWING.write()).await;
}

impl Credentials {
    /// Renews the tokens in their own task, so a cancelled prompt or start-up
    /// request cannot drop the new tokens after the endpoint has spent ours.
    /// Running copies take turns, and each first uses what the one before it
    /// saved: its new refresh token replaces ours. Tokens count as usable
    /// until they expire or until the API has turned down `rejected`, the
    /// access token they hold.
    pub(super) async fn renew(
        &mut self,
        http: &reqwest::Client,
        rejected: Option<&str>,
    ) -> Result<()> {
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
}
