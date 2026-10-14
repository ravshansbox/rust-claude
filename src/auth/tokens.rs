use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{Credentials, Unusable, expires_at};

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const RENEWAL_MARGIN: u128 = 5 * 60 * 1000;

/// Where token requests go; tests answer them locally.
async fn token_url() -> String {
    #[cfg(test)]
    if let Some(url) = super::tests::TOKEN_URL.lock().await.clone() {
        return url;
    }
    TOKEN_URL.into()
}

/// Asks the endpoint for tokens. A renewal passes the refresh token it holds
/// as `current_refresh`, kept when the endpoint does not send a new one.
pub(super) async fn request_tokens(
    http: &reqwest::Client,
    body: Value,
    current_refresh: Option<&str>,
) -> Result<Credentials> {
    let response = http.post(token_url().await).json(&body).send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let message = format!("token request failed ({status}): {text}");
        // Other client errors, such as invalid_grant, mean the endpoint
        // turned the request down; a timeout or rate limit may pass.
        if status.is_client_error()
            && status != reqwest::StatusCode::REQUEST_TIMEOUT
            && status != reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(Unusable(message).into());
        }
        bail!(message);
    }
    let data: Value = serde_json::from_str(&text)?;
    Ok(Credentials {
        access: data["access_token"]
            .as_str()
            .context("missing access_token")?
            .into(),
        refresh: data["refresh_token"]
            .as_str()
            .or(current_refresh)
            .context("missing refresh_token")?
            .into(),
        expires: expires_at(data["expires_in"].as_u64(), RENEWAL_MARGIN).unwrap_or(u64::MAX.into()),
        renewal_notice: None,
    })
}
