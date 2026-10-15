use std::{collections::BTreeMap, time::Duration};

use super::super::callback::{page, read_target, reply_page};
use super::super::{request_tokens, save};
use super::Pending;

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

impl Pending {
    /// Waits for the browser to come back with a code, trades it for tokens
    /// and saves them.
    pub async fn finish(self) -> Result<(), String> {
        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, self.wait_for_code())
            .await
            .map_err(|_| "sign-in timed out after 5 minutes".to_string())??;
        let saved = request_tokens(
            &self.client,
            &self.template,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &self.redirect_uri),
                ("code_verifier", &self.verifier),
            ],
        )
        .await?;
        save(&self.server_url, &saved)
    }

    async fn wait_for_code(&self) -> Result<String, String> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|error| format!("cannot receive the sign-in: {error}"))?;
            let Ok(Some(target)) =
                tokio::time::timeout(REQUEST_TIMEOUT, read_target(&mut stream)).await
            else {
                continue;
            };
            let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
                continue;
            };
            if url.path() != "/callback" {
                reply_page(&mut stream, "404 Not Found", "").await;
                continue;
            }
            let mut parameters: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
            let result = if parameters.get("state") != Some(&self.state) {
                Err("the sign-in page sent back a different state".to_string())
            } else if let Some(error) = parameters.remove("error") {
                Err(match parameters.get("error_description") {
                    Some(description) => format!("{error}: {description}"),
                    None => error,
                })
            } else {
                parameters
                    .remove("code")
                    .ok_or_else(|| "the sign-in page sent back no code".to_string())
            };
            let (status, message) = match &result {
                Ok(_) => (
                    "200 OK",
                    "Signed in. You can close this tab and go back to Claude Code.".to_string(),
                ),
                Err(reason) => ("400 Bad Request", format!("Sign-in failed: {reason}")),
            };
            reply_page(&mut stream, status, &page(&message)).await;
            return result;
        }
    }
}
