use std::{collections::BTreeMap, time::Duration};

use super::callback::{page, read_target, reply_page};
use super::discovery::discover;
use super::{Saved, request_tokens, save, send};
use crate::auth::{pkce_challenge, random_token};
use crate::mcp::config_file::{OAuthConfig, expand_variables};
use tokio::net::TcpListener;

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// A sign-in waiting for the browser to come back to `redirect_uri`.
pub struct Pending {
    pub authorize_url: String,
    client: reqwest::Client,
    server_url: String,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    verifier: String,
    template: Saved,
}

async fn register(
    client: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let failed =
        |reason: String| format!("cannot register Claude Code with the sign-in server: {reason}");
    let (status, reply, body) = send(client.post(registration_endpoint).json(&serde_json::json!({
        "client_name": "Claude Code",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })))
    .await
    .map_err(|error| failed(error.to_string()))?;
    match reply["client_id"].as_str() {
        Some(client_id) if status.is_success() => Ok((
            client_id.to_string(),
            reply["client_secret"].as_str().map(str::to_string),
        )),
        _ => Err(failed(format!("status {}: {body}", status.as_u16()))),
    }
}

/// Finds the sign-in server, registers with it unless `oauth` names a
/// client, and returns the page to open in the browser.
pub(in crate::mcp) async fn begin(
    client: &reqwest::Client,
    server_url: &str,
    oauth: Option<&OAuthConfig>,
) -> Result<Pending, String> {
    let discovered = discover(client, server_url).await?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("cannot listen for the sign-in: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let client_id = oauth.and_then(|oauth| oauth.client_id.as_ref());
    let client_secret = oauth.and_then(|oauth| oauth.client_secret.as_deref());
    let secret_from_config = client_id.is_some() && client_secret.is_some();
    let (client_id, client_secret) = match client_id {
        Some(client_id) => {
            let client_secret =
                client_secret
                    .map(expand_variables)
                    .transpose()
                    .map_err(|variable| {
                        format!("oauth clientSecret uses {variable}, which is not set")
                    })?;
            (client_id.clone(), client_secret)
        }
        None => {
            let registration_endpoint = discovered.registration_endpoint.as_deref().ok_or(
                "the sign-in server does not let rust-coding-agent register; set oauth.clientId in mcp.json",
            )?;
            register(client, registration_endpoint, &redirect_uri).await?
        }
    };
    let state = random_token().map_err(|error| error.to_string())?;
    let verifier = random_token().map_err(|error| error.to_string())?;
    let challenge = pkce_challenge(&verifier);
    let authorize_url = reqwest::Url::parse_with_params(
        &discovered.authorization_endpoint,
        [
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("resource", &discovered.resource),
        ]
        .into_iter()
        .chain(discovered.scope.as_deref().map(|scope| ("scope", scope))),
    )
    .map_err(|error| format!("invalid authorization endpoint: {error}"))?;
    Ok(Pending {
        authorize_url: authorize_url.to_string(),
        client: client.clone(),
        server_url: server_url.to_string(),
        listener,
        redirect_uri,
        state,
        verifier,
        template: Saved {
            client_id,
            client_secret,
            token_endpoint: discovered.token_endpoint,
            resource: Some(discovered.resource),
            secret_from_config,
            ..Default::default()
        },
    })
}

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
