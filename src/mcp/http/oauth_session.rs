use std::{fmt::Write, sync::Mutex};

use super::{Http, PostError, get, set};
use crate::mcp::{NEEDS_SIGN_IN, oauth};

pub(super) struct OAuthSession {
    pub(super) server_name: String,
    pub(super) saved: Mutex<Option<oauth::Saved>>,
    pub(super) refreshing: tokio::sync::Mutex<()>,
    /// What to tell the user about the last renewal, until it is shown.
    pub(super) renewal_notice: Mutex<Option<String>>,
}

impl OAuthSession {
    pub(super) fn current(&self) -> Option<oauth::Saved> {
        get(&self.saved)
    }

    pub(super) fn needs_sign_in(&self, reason: Option<String>) -> PostError {
        let mut message = format!("{NEEDS_SIGN_IN}: run /mcp login {}", self.server_name);
        if let Some(reason) = reason {
            let _ = write!(message, " (could not renew the sign-in: {reason})");
        }
        PostError::fatal(message)
    }
}

impl Http {
    /// The access token to send, renewed when it has expired or after the
    /// server turned down `rejected`.
    pub(super) async fn access_token(
        &self,
        rejected: Option<&str>,
    ) -> Result<Option<String>, PostError> {
        let Some(oauth) = &self.oauth else {
            return Ok(None);
        };
        let Some(current) = oauth.current() else {
            return Ok(None);
        };
        if rejected.is_none() && !current.expired() {
            return Ok(Some(current.access_token));
        }
        let _refreshing = oauth.refreshing.lock().await;
        let Some(current) = oauth.current() else {
            return Ok(None);
        };
        if Some(current.access_token.as_str()) != rejected && !current.expired() {
            return Ok(Some(current.access_token));
        }
        let renewed = oauth::refresh(&self.client, &self.url, &current, rejected)
            .await
            .map_err(|error| oauth.needs_sign_in(Some(error)))?;
        if let Some(error) = renewed.save_error {
            let notice = format!(
                "renewed the sign-in to MCP server {}, but {error}",
                oauth.server_name
            );
            set(&oauth.renewal_notice, Some(notice));
        }
        set(&oauth.saved, Some(renewed.saved.clone()));
        Ok(Some(renewed.saved.access_token))
    }
}
