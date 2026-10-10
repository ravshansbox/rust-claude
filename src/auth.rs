use std::{
    fs::{File, TryLockError},
    io::Write,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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
    Ok(crate::config::dir()
        .context("HOME is not set")?
        .join("auth.json"))
}

/// Locks auth.lock until the returned file is closed, so running copies
/// renew the sign-in one at a time.
async fn lock_credentials() -> Result<File> {
    let path = credentials_path()?.with_file_name("auth.lock");
    crate::config::create_private_dir(path.parent().context("invalid credentials path")?)?;
    let file = crate::config::private_file()
        .create(true)
        .write(true)
        .open(&path)?;
    // Waiting without a blocking lock keeps quitting from waiting for
    // another copy's renewal.
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// The saved sign-in cannot be used any more, so only signing in again helps:
/// auth.json is not valid, or the token endpoint turned the request down.
#[derive(Debug)]
struct Unusable(String);

impl std::fmt::Display for Unusable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
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
                // A rust-claude that does not take turns may have renewed
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
            Err(error) if error.is::<Unusable>() => {
                Err(error.context(format!("{problem}: restart rust-claude to sign in again")))
            }
            Err(error) => Err(crate::agent::retry::retryable(
                format!("could not renew the sign-in: {error:#}"),
                None,
            )),
        }
    }

    /// Switches to the tokens in auth.json when they last longer than ours.
    fn adopt_saved(&mut self) {
        let saved = credentials_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Self>(&text).ok());
        if let Some(saved) = saved
            && saved.expires > self.expires
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
        crate::config::create_private_dir(path.parent().context("invalid credentials path")?)?;
        crate::config::write_private_file(&path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

async fn login(http: &reqwest::Client) -> Result<Credentials> {
    let verifier = random_token()?;
    let state = random_token()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

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
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

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

fn random_token() -> Result<String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(random))
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
    let credentials = Credentials {
        access: data["access_token"]
            .as_str()
            .context("missing access_token")?
            .into(),
        refresh: data["refresh_token"]
            .as_str()
            .or(current_refresh)
            .context("missing refresh_token")?
            .into(),
        expires: now_millis() + data["expires_in"].as_u64().unwrap_or(0) as u128 * 1000
            - 5 * 60 * 1000,
        renewal_notice: None,
    };
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::{Credentials, credentials_path, exchange_code, finish_renewal, parse_pasted_code};
    use crate::agent::{
        AgentEvent,
        retry::retry_delay,
        test_support::{self, MockApi, Reply, text_reply},
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::{Mutex, Notify},
    };

    /// Tests share one auth.json, so they take turns.
    static AUTH_FILE: Mutex<()> = Mutex::const_new(());

    /// Where the running test answers token requests.
    pub(super) static TOKEN_URL: Mutex<Option<String>> = Mutex::const_new(None);

    fn unreachable_client() -> reqwest::Client {
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
            .build()
            .unwrap()
    }

    /// A local stand-in for the token endpoint. Like the real one, it accepts
    /// only the refresh token it issued last, once, and answers with a new
    /// pair.
    struct TokenEndpoint {
        /// The refresh tokens sent to it.
        received: Arc<Mutex<Vec<String>>>,
        /// Signalled when a request arrives.
        arrived: Arc<Notify>,
        /// Lets a held reply go, when the endpoint is slow.
        release: Arc<Notify>,
    }

    impl TokenEndpoint {
        async fn start(refresh: &str, slow: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            *TOKEN_URL.lock().await = Some(format!(
                "http://{}/v1/oauth/token",
                listener.local_addr().unwrap()
            ));
            let endpoint = Self {
                received: Arc::default(),
                arrived: Arc::default(),
                release: Arc::default(),
            };
            let valid = Arc::new(Mutex::new(refresh.to_string()));
            let (received, arrived, release) = (
                endpoint.received.clone(),
                endpoint.arrived.clone(),
                endpoint.release.clone(),
            );
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let (received, arrived, release, valid) = (
                        received.clone(),
                        arrived.clone(),
                        release.clone(),
                        valid.clone(),
                    );
                    tokio::spawn(async move {
                        let body = read_body(&mut stream).await;
                        let token = body["refresh_token"].as_str().unwrap_or_default();
                        received.lock().await.push(token.to_string());
                        arrived.notify_one();
                        if slow {
                            release.notified().await;
                        }
                        let reply = {
                            let mut valid = valid.lock().await;
                            (token == *valid).then(|| {
                                *valid = format!("{token}+");
                                json!({
                                    "access_token": format!("access from {token}"),
                                    "refresh_token": *valid,
                                    "expires_in": 3600,
                                })
                            })
                        };
                        let (status, body) = match reply {
                            Some(body) => ("200 OK", body),
                            None => ("400 Bad Request", json!({ "error": "invalid_grant" })),
                        };
                        respond(&mut stream, status, &body).await;
                    });
                }
            });
            endpoint
        }
    }

    /// A token endpoint that gives every request the same answer.
    async fn answer_token_requests(status: &'static str, body: Value) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        *TOKEN_URL.lock().await = Some(format!(
            "http://{}/v1/oauth/token",
            listener.local_addr().unwrap()
        ));
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    read_body(&mut stream).await;
                    respond(&mut stream, status, &body).await;
                });
            }
        });
    }

    async fn respond(stream: &mut TcpStream, status: &str, body: &Value) {
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    }

    async fn read_body(stream: &mut TcpStream) -> Value {
        let mut data = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..end]).to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    return serde_json::from_slice(&data[end + 4..]).unwrap_or(Value::Null);
                }
            }
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return Value::Null,
                Ok(read) => data.extend_from_slice(&chunk[..read]),
            }
        }
    }

    fn expired(refresh: &str) -> Credentials {
        serde_json::from_value(json!({ "access": "old", "refresh": refresh, "expires": 0 }))
            .unwrap()
    }

    #[tokio::test]
    async fn keeps_the_saved_sign_in_when_the_token_endpoint_is_down() {
        let _lock = AUTH_FILE.lock().await;
        answer_token_requests("503 Service Unavailable", json!({ "error": "overloaded" })).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let saved = json!({ "access": "old", "refresh": "first", "expires": 0 }).to_string();
        std::fs::write(&path, &saved).unwrap();
        let result = Credentials::load_or_login(&reqwest::Client::new()).await;
        let after = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let error = format!(
            "{:#}",
            result.err().expect("signed in without the endpoint")
        );
        assert!(error.contains("503"), "{error}");
        assert_eq!(after, saved);
    }

    #[tokio::test]
    async fn retries_a_renewal_the_server_could_not_answer() {
        let _lock = AUTH_FILE.lock().await;
        answer_token_requests("503 Service Unavailable", json!({ "error": "overloaded" })).await;
        let _ = std::fs::remove_file(credentials_path().unwrap());
        let mut credentials = expired("first");
        let error = credentials
            .access_token(&reqwest::Client::new())
            .await
            .unwrap_err();
        assert!(retry_delay(&error, 0).is_some(), "{error:#}");
        assert!(error.to_string().contains("503"), "{error}");
    }

    #[tokio::test]
    async fn asks_to_sign_in_again_when_the_server_turns_the_renewal_down() {
        let _lock = AUTH_FILE.lock().await;
        let _endpoint = TokenEndpoint::start("other", false).await;
        let _ = std::fs::remove_file(credentials_path().unwrap());
        let mut credentials = expired("spent");
        let error = credentials
            .access_token(&reqwest::Client::new())
            .await
            .unwrap_err();
        assert_eq!(retry_delay(&error, 0), None);
        assert!(error.to_string().contains("sign in again"), "{error}");
    }

    #[tokio::test]
    async fn keeps_the_refresh_token_when_a_renewal_does_not_replace_it() {
        let _lock = AUTH_FILE.lock().await;
        answer_token_requests(
            "200 OK",
            json!({ "access_token": "new", "expires_in": 3600 }),
        )
        .await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let mut credentials = expired("first");
        let token = credentials.access_token(&reqwest::Client::new()).await;
        let saved = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(token.unwrap(), "new");
        let saved: Credentials = serde_json::from_str(&saved.unwrap()).unwrap();
        assert_eq!(saved.refresh, "first");
    }

    #[tokio::test]
    async fn keeps_a_new_sign_in_that_could_not_be_saved() {
        let _lock = AUTH_FILE.lock().await;
        answer_token_requests(
            "200 OK",
            json!({ "access_token": "new", "refresh_token": "next", "expires_in": 3600 }),
        )
        .await;
        let path = credentials_path().unwrap();
        // A folder in the way of auth.json makes saving fail.
        std::fs::create_dir_all(&path).unwrap();
        let credentials = exchange_code(&reqwest::Client::new(), "code", "state", "verifier").await;
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(credentials.unwrap().access, "new");
    }

    #[tokio::test]
    async fn keeps_a_renewed_sign_in_that_could_not_be_saved() {
        let _lock = AUTH_FILE.lock().await;
        let _endpoint = TokenEndpoint::start("first", false).await;
        let path = credentials_path().unwrap();
        // A folder in the way of auth.json makes saving fail.
        std::fs::create_dir_all(&path).unwrap();
        let mut credentials = expired("first");
        let token = credentials.access_token(&reqwest::Client::new()).await;
        let notice = credentials.take_renewal_notice();
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(token.unwrap(), "access from first");
        let notice = notice.unwrap();
        assert!(notice.contains("could not save"), "{notice}");
    }

    #[tokio::test]
    async fn finishes_renewing_after_the_caller_gives_up() {
        let _lock = AUTH_FILE.lock().await;
        let endpoint = TokenEndpoint::start("first", true).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let prompt = tokio::spawn(async {
            let mut credentials = expired("first");
            credentials.access_token(&reqwest::Client::new()).await
        });
        endpoint.arrived.notified().await;
        prompt.abort();
        let _ = prompt.await;
        endpoint.release.notify_one();
        let mut saved = None;
        for _ in 0..100 {
            saved = std::fs::read_to_string(&path).ok();
            if saved.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let _ = std::fs::remove_file(&path);
        let saved: Credentials =
            serde_json::from_str(&saved.expect("auth.json not saved")).unwrap();
        assert_eq!(
            (saved.access.as_str(), saved.refresh.as_str()),
            ("access from first", "first+")
        );
    }

    #[tokio::test]
    async fn quitting_waits_for_a_renewal_to_be_saved() {
        let _lock = AUTH_FILE.lock().await;
        let endpoint = TokenEndpoint::start("first", true).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let prompt = tokio::spawn(async {
            let mut credentials = expired("first");
            credentials.access_token(&reqwest::Client::new()).await
        });
        endpoint.arrived.notified().await;
        prompt.abort();
        let _ = prompt.await;
        let release = endpoint.release.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            release.notify_one();
        });
        finish_renewal(std::time::Duration::from_secs(5)).await;
        let saved = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let saved: Credentials =
            serde_json::from_str(&saved.expect("auth.json not saved")).unwrap();
        assert_eq!(saved.refresh, "first+");
    }

    #[tokio::test]
    async fn quitting_stops_waiting_for_a_renewal_that_hangs() {
        let _lock = AUTH_FILE.lock().await;
        let endpoint = TokenEndpoint::start("first", true).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let prompt = tokio::spawn(async {
            let mut credentials = expired("first");
            credentials.access_token(&reqwest::Client::new()).await
        });
        endpoint.arrived.notified().await;
        let waited = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            finish_renewal(std::time::Duration::from_millis(100)),
        )
        .await;
        endpoint.release.notify_one();
        let _ = prompt.await;
        let _ = std::fs::remove_file(&path);
        assert!(waited.is_ok(), "kept waiting for the renewal");
    }

    #[tokio::test]
    async fn renews_a_sign_in_the_api_turned_down_and_sends_the_request_again() {
        let _lock = AUTH_FILE.lock().await;
        let endpoint = TokenEndpoint::start("refresh", false).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut text = String::new();
        let result = agent
            .prompt("hi", &[], |event| {
                if let AgentEvent::Text(delta) = event {
                    text.push_str(&delta);
                }
            })
            .await;
        test_support::remove_session(&agent);
        let _ = std::fs::remove_file(&path);
        result.unwrap();
        assert_eq!(text, "hello");
        assert_eq!(*endpoint.received.lock().await, ["refresh"]);
        let headers = api.headers().await;
        assert_eq!(headers.len(), 2);
        assert!(
            headers[1].contains("authorization: bearer access from refresh"),
            "{}",
            headers[1]
        );
    }

    #[tokio::test]
    async fn uses_a_sign_in_another_instance_renewed_after_the_api_turned_ours_down() {
        let _lock = AUTH_FILE.lock().await;
        let endpoint = TokenEndpoint::start("refresh", false).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_001u64 })
                .to_string(),
        )
        .unwrap();
        let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let result = agent.prompt("hi", &[], |_| {}).await;
        test_support::remove_session(&agent);
        let _ = std::fs::remove_file(&path);
        result.unwrap();
        assert!(endpoint.received.lock().await.is_empty());
        let headers = api.headers().await;
        assert_eq!(headers.len(), 2);
        assert!(
            headers[1].contains("authorization: bearer renewed"),
            "{}",
            headers[1]
        );
    }

    #[tokio::test]
    async fn asks_to_sign_in_again_when_a_sign_in_the_api_turned_down_cannot_be_renewed() {
        let _lock = AUTH_FILE.lock().await;
        let _endpoint = TokenEndpoint::start("other", false).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let api = MockApi::start(vec![Reply::Unauthorized, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let result = agent.prompt("hi", &[], |_| {}).await;
        test_support::remove_session(&agent);
        // The next start reads what was saved and, unable to renew it, signs
        // in again instead of sending the turned-down token.
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        let next_start = Credentials::load_and_refresh(&saved, &reqwest::Client::new()).await;
        let _ = std::fs::remove_file(&path);
        let error = result.unwrap_err();
        assert!(error.to_string().contains("sign in again"), "{error}");
        assert_eq!(api.requests().await.len(), 1);
        assert!(next_start.is_err_and(|error| error.is::<super::Unusable>()));
    }

    #[tokio::test]
    async fn running_copies_renew_one_at_a_time() {
        let _lock = AUTH_FILE.lock().await;
        let endpoint = TokenEndpoint::start("first", false).await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({ "access": "old", "refresh": "first", "expires": 0 }).to_string(),
        )
        .unwrap();
        let http = reqwest::Client::new();
        let (mut one, mut other) = (expired("first"), expired("first"));
        let (token, other_token) = tokio::join!(one.access_token(&http), other.access_token(&http));
        let _ = std::fs::remove_file(&path);
        assert_eq!(*endpoint.received.lock().await, ["first"]);
        assert_eq!(token.unwrap(), "access from first");
        assert_eq!(other_token.unwrap(), "access from first");
    }

    #[tokio::test]
    async fn uses_a_sign_in_another_instance_renewed_after_startup() {
        let _lock = AUTH_FILE.lock().await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_000u64 })
                .to_string(),
        )
        .unwrap();
        let read_at_startup =
            json!({ "access": "old", "refresh": "spent", "expires": 0 }).to_string();
        let credentials =
            Credentials::load_and_refresh(&read_at_startup, &unreachable_client()).await;
        let _ = std::fs::remove_file(&path);
        assert_eq!(credentials.unwrap().access, "renewed");
    }

    #[tokio::test]
    async fn uses_a_token_another_instance_renewed() {
        let _lock = AUTH_FILE.lock().await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({ "access": "renewed", "refresh": "new", "expires": 4_102_444_800_000u64 })
                .to_string(),
        )
        .unwrap();
        let mut credentials: Credentials =
            serde_json::from_value(json!({ "access": "old", "refresh": "spent", "expires": 0 }))
                .unwrap();
        let token = credentials.access_token(&reqwest::Client::new()).await;
        let _ = std::fs::remove_file(&path);
        assert_eq!(token.unwrap(), "renewed");
    }

    #[tokio::test]
    async fn saving_never_leaves_a_reader_with_a_partial_sign_in() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let _lock = AUTH_FILE.lock().await;
        let path = credentials_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = json!({ "access": "old", "refresh": "old", "expires": 1 }).to_string();
        std::fs::write(&path, &old).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();

        let new: Credentials =
            serde_json::from_value(json!({ "access": "new", "refresh": "new", "expires": 2 }))
                .unwrap();
        new.save().unwrap();

        let mut seen_by_reader = String::new();
        reader.read_to_string(&mut seen_by_reader).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let saved: Credentials =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let leftovers = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|entry| {
                let name = entry.as_ref().unwrap().file_name();
                name.to_string_lossy().starts_with("auth.json.")
            })
            .count();
        let _ = std::fs::remove_file(&path);
        assert_eq!(seen_by_reader, old);
        assert_eq!(mode, 0o600);
        assert_eq!((saved.access.as_str(), saved.expires), ("new", 2));
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn accepts_a_pasted_code_with_the_state_we_sent() {
        assert_eq!(parse_pasted_code(" abc#xyz\n", "xyz").unwrap(), "abc");
    }

    #[test]
    fn rejects_a_pasted_code_with_another_state() {
        assert!(parse_pasted_code("abc#other", "xyz").is_err());
    }

    #[test]
    fn rejects_a_pasted_code_without_its_state() {
        let error = parse_pasted_code("abc\n", "xyz").unwrap_err();
        assert!(error.to_string().contains("whole code"), "{error}");
    }
}
