use std::{
    fmt,
    fs::File,
    io::{self, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{agent::retry::retryable, config};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

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

async fn login(http: &reqwest::Client) -> Result<Credentials> {
    let verifier = random_token()?;
    let state = random_token()?;
    let challenge = pkce_challenge(&verifier);

    let url = reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        [
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
        ],
    )?;

    eprintln!("Open this URL and sign in with your Claude Pro/Max account:\n\n{url}\n");
    eprint!("Paste the code: ");
    io::stderr().flush()?;
    // Read off the runtime, so a stop signal is handled while waiting.
    let input = tokio::task::spawn_blocking(|| {
        let mut input = String::new();
        io::stdin().read_line(&mut input).map(|_| input)
    })
    .await??;

    let code = parse_pasted_code(&input, &state)?;
    exchange_code(http, code, &state, &verifier).await
}

/// Trades the pasted code for tokens and saves them. The code works only
/// once, so the new tokens are kept even when auth.json cannot take them.
async fn exchange_code(
    http: &reqwest::Client,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<Credentials> {
    let credentials = request_tokens(
        http,
        json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        }),
        None,
    )
    .await?;
    if let Err(error) = credentials.save() {
        eprintln!("Signed in, but could not save the sign-in to auth.json: {error:#}\n");
    }
    Ok(credentials)
}

pub(crate) fn random_token() -> Result<String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Splits the `code#state` text the callback page shows and checks the
/// state against the one we sent. The page always includes the state, so
/// text without it was not copied whole.
fn parse_pasted_code<'a>(input: &'a str, expected_state: &str) -> Result<&'a str> {
    let Some((code, state)) = input.trim().split_once('#') else {
        bail!("the pasted code has no #state part; paste the whole code shown after sign-in");
    };
    if state != expected_state {
        bail!("OAuth state mismatch");
    }
    Ok(code)
}

/// Where token requests go; tests answer them locally.
async fn token_url() -> String {
    #[cfg(test)]
    if let Some(url) = tests::TOKEN_URL.lock().await.clone() {
        return url;
    }
    TOKEN_URL.into()
}

/// Asks the endpoint for tokens. A renewal passes the refresh token it holds
/// as `current_refresh`, kept when the endpoint does not send a new one.
async fn request_tokens(
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

pub(crate) fn expires_at(seconds: Option<u64>, margin: u128) -> Option<u128> {
    let lifetime = u128::from(seconds?) * 1000;
    Some(now_millis() + lifetime - lifetime.min(2 * margin) / 2)
}

const RENEWAL_MARGIN: u128 = 5 * 60 * 1000;

#[cfg(test)]
pub(crate) mod tests;
