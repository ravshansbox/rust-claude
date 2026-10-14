use std::{collections::BTreeMap, time::Duration};

use crate::{
    auth::{expires_at, pkce_challenge, random_token},
    skills::escape_xml,
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

mod saved;

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
        ..saved.clone()
    })
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
        current.client_secret = saved.client_secret.clone();
        current.secret_from_config = true;
    }
    Some(current)
}

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// What the sign-in server needs, found from the MCP server.
struct Discovered {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    resource: String,
    scope: Option<String>,
}

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

/// Reads a `key="value"` or `key=value` parameter of a `WWW-Authenticate`
/// challenge, skipping text inside quoted values.
fn challenge_parameter(challenge: &str, key: &str) -> Option<String> {
    let mut quoted = false;
    let mut escaped = false;
    let mut before = None;
    for (start, character) in challenge.char_indices() {
        let at_key = !quoted
            && !before.is_some_and(|before: char| before.is_ascii_alphanumeric() || before == '_');
        before = Some(character);
        if quoted {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        if character == '"' {
            quoted = true;
            continue;
        }
        let Some(value) = challenge
            .get(start..start + key.len())
            .filter(|name| at_key && name.eq_ignore_ascii_case(key))
            .map(|_| &challenge[start + key.len()..])
            .and_then(|after| after.trim_start().strip_prefix('='))
        else {
            continue;
        };
        let value = value.trim_start();
        return Some(match value.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().unwrap_or_default().to_string(),
            None => value
                .split([',', ' '])
                .next()
                .unwrap_or_default()
                .to_string(),
        });
    }
    None
}

/// `{origin}/.well-known/{name}{path}`, as RFC 8414 and RFC 9728 place
/// metadata for a URL with a path.
fn well_known(url: &reqwest::Url, name: &str) -> String {
    let path = url.path().trim_end_matches('/');
    format!(
        "{}/.well-known/{name}{path}",
        url.origin().ascii_serialization()
    )
}

async fn first_json(
    client: &reqwest::Client,
    urls: &[String],
    check: impl Fn(&Value) -> bool,
) -> Option<Value> {
    for url in urls {
        let request = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Ok(response) = request.send().await
            && response.status().is_success()
            && let Ok(metadata) = response.json::<Value>().await
            && check(&metadata)
        {
            return Some(metadata);
        }
    }
    None
}

async fn discover(client: &reqwest::Client, server_url: &str) -> Result<Discovered, String> {
    if !secure_endpoint(server_url) {
        return Err(format!("the MCP server does not use HTTPS: {server_url}"));
    }
    let url = reqwest::Url::parse(server_url).map_err(|error| error.to_string())?;
    let probe = client
        .post(server_url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": super::initialize_params(),
        }))
        .send()
        .await
        .map_err(|error| format!("cannot reach {server_url}: {error}"))?;
    let challenge = probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let origin = url.origin().ascii_serialization();
    let resource_urls: Vec<String> = challenge_parameter(&challenge, "resource_metadata")
        .into_iter()
        .chain([
            well_known(&url, "oauth-protected-resource"),
            format!("{origin}/.well-known/oauth-protected-resource"),
        ])
        .collect();
    let resource_metadata = first_json(client, &resource_urls, |metadata| {
        metadata["authorization_servers"][0].is_string()
    })
    .await;
    let issuer = match &resource_metadata {
        Some(metadata) => metadata["authorization_servers"][0]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        None => origin,
    };
    let resource = resource_metadata
        .as_ref()
        .and_then(|metadata| metadata["resource"].as_str());
    if let Some(resource) = resource
        && !covers(resource, &url)
    {
        return Err(format!(
            "the sign-in metadata of {server_url} is for another server: {resource}"
        ));
    }
    let issuer_url = reqwest::Url::parse(&issuer)
        .map_err(|error| format!("invalid sign-in server {issuer:?}: {error}"))?;
    let mut metadata_urls = vec![
        well_known(&issuer_url, "oauth-authorization-server"),
        well_known(&issuer_url, "openid-configuration"),
    ];
    if !issuer_url.path().trim_end_matches('/').is_empty() {
        metadata_urls.push(format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ));
    }
    let Some(server_metadata) = first_json(client, &metadata_urls, |metadata| {
        metadata["authorization_endpoint"].is_string() && metadata["token_endpoint"].is_string()
    })
    .await
    else {
        return Err(format!("cannot find the sign-in server for {server_url}"));
    };
    if resource_metadata.is_some()
        && let Some(named) = server_metadata["issuer"].as_str()
        && named.trim_end_matches('/') != issuer.trim_end_matches('/')
    {
        return Err(format!(
            "the sign-in server {issuer} names another issuer: {named}"
        ));
    }
    for kind in ["authorization", "token"] {
        let endpoint = server_metadata[format!("{kind}_endpoint")]
            .as_str()
            .unwrap_or_default();
        if !secure_endpoint(endpoint) {
            return Err(format!(
                "the sign-in server's {kind} endpoint does not use HTTPS: {endpoint}"
            ));
        }
    }
    let text = |value: &Value| value.as_str().map(str::to_string);
    let scope = challenge_parameter(&challenge, "scope").or_else(|| {
        let scopes: Vec<&str> = resource_metadata
            .as_ref()?
            .get("scopes_supported")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        (!scopes.is_empty()).then(|| scopes.join(" "))
    });
    Ok(Discovered {
        authorization_endpoint: text(&server_metadata["authorization_endpoint"])
            .unwrap_or_default(),
        token_endpoint: text(&server_metadata["token_endpoint"]).unwrap_or_default(),
        registration_endpoint: text(&server_metadata["registration_endpoint"]),
        resource: resource.unwrap_or(server_url).to_string(),
        scope,
    })
}

/// Whether `resource` is `url`, or its origin or a parent path of it.
fn covers(resource: &str, url: &reqwest::Url) -> bool {
    let Ok(resource) = reqwest::Url::parse(resource) else {
        return false;
    };
    let parent = resource.path().trim_end_matches('/');
    let path = url.path();
    resource.origin() == url.origin()
        && (path.trim_end_matches('/') == parent
            || path
                .strip_prefix(parent)
                .is_some_and(|rest| rest.starts_with('/')))
}

fn secure_endpoint(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match url.scheme() {
        "https" => true,
        "http" => {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        }
        _ => false,
    }
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
    oauth: Option<&super::config_file::OAuthConfig>,
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
            let client_secret = client_secret
                .map(super::config_file::expand_variables)
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
                    "Signed in. You can close this tab and go back to rust-coding-agent."
                        .to_string(),
                ),
                Err(reason) => ("400 Bad Request", format!("Sign-in failed: {reason}")),
            };
            reply_page(&mut stream, status, &page(&message)).await;
            return result;
        }
    }
}
