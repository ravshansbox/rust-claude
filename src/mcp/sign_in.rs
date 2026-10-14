use std::time::Duration;

use super::{
    SignIn,
    config_file::{ServerConfig, http_url_of, uses_oauth},
    configured, oauth,
};

pub(super) async fn begin_sign_in_with(
    name: &str,
    config: &ServerConfig,
) -> Result<SignIn, String> {
    let url = http_url_of(name, config)?;
    if !uses_oauth(config) {
        return Err(format!(
            "MCP server {name} sends its own Authorization header"
        ));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("cannot set up HTTP: {error}"))?;
    oauth::begin(&client, url, config.oauth.as_ref()).await
}

pub(super) fn sign_out_with(name: &str, config: &ServerConfig) -> Result<String, String> {
    let url = http_url_of(name, config)?;
    Ok(if oauth::remove(url)? {
        format!("signed out of MCP server {name}")
    } else {
        format!("MCP server {name} was not signed in")
    })
}

/// Forgets the saved sign-in for the configured server `name`.
pub fn sign_out(name: &str) -> Result<String, String> {
    sign_out_with(name, &configured(name)?)
}

/// Starts signing in to the configured server `name`.
pub async fn begin_sign_in(name: &str) -> Result<SignIn, String> {
    begin_sign_in_with(name, &configured(name)?).await
}
