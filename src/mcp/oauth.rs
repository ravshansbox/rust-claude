use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Tokens are renewed this long before they expire.
const EXPIRY_MARGIN: u128 = 60_000;

/// Serialises changes to mcp-auth.json within this process.
static FILE: Mutex<()> = Mutex::new(());

/// The sign-in for one server URL, as saved in mcp-auth.json.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Saved {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the access token expires, in milliseconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u128>,
}

impl Saved {
    pub fn expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| now_millis() + EXPIRY_MARGIN >= expires_at)
    }
}

pub(super) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn path() -> Option<PathBuf> {
    Some(crate::config::dir()?.join("mcp-auth.json"))
}

fn read_all() -> BTreeMap<String, Saved> {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_all(saved: &BTreeMap<String, Saved>) -> Result<(), String> {
    let path = path().ok_or("HOME is not set")?;
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            crate::config::create_private_dir(parent)?;
        }
        let text = serde_json::to_string_pretty(saved)?;
        crate::config::write_private_file(&path, text.as_bytes())
    };
    write().map_err(|error| format!("cannot save {}: {error}", path.display()))
}

pub(super) fn load(server_url: &str) -> Option<Saved> {
    let _file = FILE.lock();
    read_all().remove(server_url)
}

pub(super) fn save(server_url: &str, saved: &Saved) -> Result<(), String> {
    let _file = FILE.lock();
    let mut all = read_all();
    all.insert(server_url.to_string(), saved.clone());
    write_all(&all)
}

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
    let response = client
        .post(&saved.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("cannot reach {}: {error}", saved.token_endpoint))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let reply: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let Some(access_token) = reply["access_token"]
        .as_str()
        .filter(|_| status.is_success())
    else {
        let reason = reply["error_description"]
            .as_str()
            .or(reply["error"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| body.trim().chars().take(200).collect());
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
        expires_at: reply["expires_in"]
            .as_u64()
            .map(|seconds| now_millis() + u128::from(seconds) * 1000),
        ..saved.clone()
    })
}

/// Renews `saved` with its refresh token and saves the new tokens. Tokens
/// another copy of rust-claude saved meanwhile are used instead, unless they
/// are the ones the server turned down.
pub(super) async fn refresh(
    client: &reqwest::Client,
    server_url: &str,
    saved: &Saved,
    rejected: Option<&str>,
) -> Result<Saved, String> {
    if let Some(current) = load(server_url)
        && current.access_token != saved.access_token
        && Some(current.access_token.as_str()) != rejected
        && !current.expired()
    {
        return Ok(current);
    }
    let refresh_token = saved.refresh_token.as_deref().ok_or("no refresh token")?;
    let renewed = request_tokens(
        client,
        saved,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    save(server_url, &renewed)?;
    Ok(renewed)
}

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// What the sign-in server needs, found from the MCP server.
struct Discovered {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    resource: String,
    scope: Option<String>,
}

/// A sign-in waiting for the browser to come back to `redirect_uri`.
pub(super) struct Pending {
    pub authorize_url: String,
    client: reqwest::Client,
    server_url: String,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    verifier: String,
    template: Saved,
}

fn random_token() -> Result<String, String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|error| error.to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

/// Reads a `key="value"` or `key=value` parameter of a `WWW-Authenticate`
/// challenge.
fn challenge_parameter(challenge: &str, key: &str) -> Option<String> {
    let mut rest = challenge;
    while let Some(start) = rest.find(key) {
        let before = rest[..start].chars().next_back();
        let after = &rest[start + key.len()..];
        rest = after;
        if before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_') {
            continue;
        }
        let Some(value) = after.trim_start().strip_prefix('=') else {
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

async fn get_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

async fn discover(client: &reqwest::Client, server_url: &str) -> Result<Discovered, String> {
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
            "params": {
                "protocolVersion": super::PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "rust-claude", "version": env!("CARGO_PKG_VERSION") },
            },
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
    let mut resource_urls = Vec::new();
    resource_urls.extend(challenge_parameter(&challenge, "resource_metadata"));
    resource_urls.push(well_known(&url, "oauth-protected-resource"));
    resource_urls.push(format!(
        "{}/.well-known/oauth-protected-resource",
        url.origin().ascii_serialization()
    ));
    let mut resource_metadata = None;
    for resource_url in &resource_urls {
        if let Some(metadata) = get_json(client, resource_url).await
            && metadata["authorization_servers"][0].is_string()
        {
            resource_metadata = Some(metadata);
            break;
        }
    }
    let issuer = match &resource_metadata {
        Some(metadata) => metadata["authorization_servers"][0]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        None => url.origin().ascii_serialization(),
    };
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
    let mut server_metadata = None;
    for metadata_url in &metadata_urls {
        if let Some(metadata) = get_json(client, metadata_url).await
            && metadata["authorization_endpoint"].is_string()
            && metadata["token_endpoint"].is_string()
        {
            server_metadata = Some(metadata);
            break;
        }
    }
    let Some(server_metadata) = server_metadata else {
        return Err(format!("cannot find the sign-in server for {server_url}"));
    };
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
        resource: resource_metadata
            .as_ref()
            .and_then(|metadata| text(&metadata["resource"]))
            .unwrap_or_else(|| server_url.to_string()),
        scope,
    })
}

async fn register(
    client: &reqwest::Client,
    registration_endpoint: &str,
    client_name: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let failed =
        |reason: String| format!("cannot register {client_name} with the sign-in server: {reason}");
    let response = client
        .post(registration_endpoint)
        .json(&serde_json::json!({
            "client_name": client_name,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .map_err(|error| failed(error.to_string()))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let reply: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    match reply["client_id"].as_str() {
        Some(client_id) if status.is_success() => Ok((
            client_id.to_string(),
            reply["client_secret"].as_str().map(str::to_string),
        )),
        _ => Err(failed(format!(
            "status {}: {}",
            status.as_u16(),
            body.trim().chars().take(200).collect::<String>()
        ))),
    }
}

/// Finds the sign-in server, registers with it unless `oauth` names a
/// client, and returns the page to open in the browser.
pub(super) async fn begin(
    client: &reqwest::Client,
    server_url: &str,
    oauth: &super::OAuthConfig,
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
    let (client_id, client_secret) = match &oauth.client_id {
        Some(client_id) => {
            let client_secret = oauth
                .client_secret
                .as_deref()
                .map(super::expand_variables)
                .transpose()
                .map_err(|variable| {
                    format!("oauth clientSecret uses {variable}, which is not set")
                })?;
            (client_id.clone(), client_secret)
        }
        None => {
            let registration_endpoint = discovered.registration_endpoint.as_deref().ok_or(
                "the sign-in server does not let rust-claude register; set oauth.clientId in the global mcp.json",
            )?;
            let client_name = oauth.client_name.as_deref().unwrap_or("rust-claude");
            register(client, registration_endpoint, client_name, &redirect_uri).await?
        }
    };
    let state = random_token()?;
    let verifier = random_token()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut authorize_url = reqwest::Url::parse(&discovered.authorization_endpoint)
        .map_err(|error| format!("invalid authorization endpoint: {error}"))?;
    {
        let mut query = authorize_url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &discovered.resource);
        if let Some(scope) = &discovered.scope {
            query.append_pair("scope", scope);
        }
    }
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
            access_token: String::new(),
            refresh_token: None,
            expires_at: None,
        },
    })
}

const SIGNED_IN_PAGE: &str = "<!doctype html><title>rust-claude</title><p>Signed in. You can close this tab and go back to rust-claude.</p>";

fn failed_page(reason: &str) -> String {
    let reason = reason
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<!doctype html><title>rust-claude</title><p>Sign-in failed: {reason}</p>")
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
            let Some(target) = read_target(&mut stream).await else {
                continue;
            };
            let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
                continue;
            };
            if url.path() != "/callback" {
                reply_page(&mut stream, "404 Not Found", "").await;
                continue;
            }
            let parameters: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
            let result = if let Some(error) = parameters.get("error") {
                Err(match parameters.get("error_description") {
                    Some(description) => format!("{error}: {description}"),
                    None => error.clone(),
                })
            } else if parameters.get("state") != Some(&self.state) {
                Err("the sign-in page sent back a different state".to_string())
            } else {
                parameters
                    .get("code")
                    .cloned()
                    .ok_or_else(|| "the sign-in page sent back no code".to_string())
            };
            match &result {
                Ok(_) => reply_page(&mut stream, "200 OK", SIGNED_IN_PAGE).await,
                Err(reason) => {
                    reply_page(&mut stream, "400 Bad Request", &failed_page(reason)).await
                }
            }
            return result;
        }
    }
}
