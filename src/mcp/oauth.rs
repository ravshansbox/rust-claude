use std::{collections::BTreeMap, time::Duration};

use super::config_file::{OAuthConfig, expand_variables};
use crate::{
    auth::{expires_at, pkce_challenge, random_token},
    skills::escape_xml,
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

mod discovery;
mod saved;

use discovery::discover;
pub(super) use saved::{Saved, load, load_for, remove, save};

/// Tokens are renewed this long before they expire, or halfway through
/// their lifetime when that is shorter than twice this.
const EXPIRY_MARGIN: u128 = 60_000;

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
    let (status, reply, body) = send(
        client
            .post(&saved.token_endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form),
    )
    .await
    .map_err(|error| format!("cannot reach {}: {error}", saved.token_endpoint))?;
    let Some(access_token) = reply["access_token"]
        .as_str()
        .filter(|_| status.is_success())
    else {
        let reason = reply["error_description"]
            .as_str()
            .or_else(|| reply["error"].as_str())
            .map_or(body, str::to_string);
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
        expires_at: expires_at(reply["expires_in"].as_u64(), EXPIRY_MARGIN),
        user: user(&reply).or_else(|| saved.user.clone()),
        ..saved.clone()
    })
}

/// The user named in the ID token, or else in the access token, when
/// either is a JWT.
fn user(reply: &Value) -> Option<String> {
    ["id_token", "access_token"]
        .into_iter()
        .filter_map(|token| jwt_claims(reply[token].as_str()?))
        .find_map(|claims| {
            ["preferred_username", "email", "name", "sub"]
                .into_iter()
                .find_map(|claim| claims[claim].as_str().filter(|value| !value.is_empty()))
                .map(str::to_string)
        })
}

fn jwt_claims(token: &str) -> Option<Value> {
    use base64::Engine;
    let [_, payload, _] = token.split('.').collect::<Vec<_>>()[..] else {
        return None;
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&payload).ok()
}

async fn send(
    request: reqwest::RequestBuilder,
) -> Result<(reqwest::StatusCode, Value, String), reqwest::Error> {
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let reply = serde_json::from_str(&body).unwrap_or(Value::Null);
    Ok((status, reply, body.trim().chars().take(200).collect()))
}

/// Tokens a renewal got, and why they could not be saved, if they could not.
pub(super) struct Renewed {
    pub saved: Saved,
    pub save_error: Option<String>,
}

/// Renews `saved` with its refresh token and saves the new tokens. Running
/// copies of rust-coding-agent take turns, and tokens another copy saved
/// meanwhile are used instead, unless they are the ones the server turned
/// down.
pub(super) async fn refresh(
    client: &reqwest::Client,
    server_url: &str,
    saved: &Saved,
    rejected: Option<&str>,
) -> Result<Renewed, String> {
    let taken_over = |saved| Renewed {
        saved,
        save_error: None,
    };
    // Without the lock, renewing still works; copies just may not take turns.
    let _lock = saved::lock_renewals().await;
    if let Some(current) = saved_meanwhile(server_url, saved, rejected) {
        return Ok(taken_over(current));
    }
    let refresh_token = saved.refresh_token.as_deref().ok_or("no refresh token")?;
    let renewed = match request_tokens(
        client,
        saved,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
    {
        Ok(renewed) => renewed,
        // A copy that does not take turns may have renewed the tokens
        // meanwhile, spending the refresh token we hold.
        Err(error) => {
            return saved_meanwhile(server_url, saved, rejected)
                .map(taken_over)
                .ok_or(error);
        }
    };
    // The endpoint has spent our refresh token, so keep the new tokens even
    // when mcp-auth.json cannot take them.
    let save_error = save(server_url, &renewed).err();
    Ok(Renewed {
        saved: renewed,
        save_error,
    })
}

/// The tokens saved for `server_url` when another copy has replaced ours
/// with ones that are neither expired nor `rejected`.
fn saved_meanwhile(server_url: &str, saved: &Saved, rejected: Option<&str>) -> Option<Saved> {
    let mut current = load(server_url).filter(|current| {
        current.access_token != saved.access_token
            && Some(current.access_token.as_str()) != rejected
            && !current.expired()
    })?;
    if saved.secret_from_config && current.client_id == saved.client_id {
        current.client_secret.clone_from(&saved.client_secret);
        current.secret_from_config = true;
    }
    Some(current)
}

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
pub(super) async fn begin(
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

fn page(message: &str) -> String {
    let message = escape_xml(message);
    format!("<!doctype html><title>rust-coding-agent</title><p>{message}</p>")
}

async fn reply_page(stream: &mut TcpStream, status: &str, page: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Reads the request line of a browser request, such as
/// `GET /callback?code=… HTTP/1.1`, and returns its target.
async fn read_target(stream: &mut TcpStream) -> Option<String> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    while !data.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || data.len() > 64 * 1024 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&data);
    head.lines().next()?.split(' ').nth(1).map(str::to_string)
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
