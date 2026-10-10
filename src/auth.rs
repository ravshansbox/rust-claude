use std::{
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
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

#[derive(Serialize, Deserialize)]
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

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

impl Credentials {
    pub async fn load_or_login(http: &reqwest::Client) -> Result<Self> {
        let Ok(text) = std::fs::read_to_string(credentials_path()?) else {
            return login(http).await;
        };
        match Self::load_and_refresh(&text, http).await {
            Ok(credentials) => Ok(credentials),
            Err(error) => {
                eprintln!("Saved sign-in is not usable ({error}). Sign in again.\n");
                login(http).await
            }
        }
    }

    async fn load_and_refresh(text: &str, http: &reqwest::Client) -> Result<Self> {
        let mut credentials: Self = serde_json::from_str(text)?;
        if now_millis() >= credentials.expires {
            credentials.renew(http).await?;
        }
        Ok(credentials)
    }

    /// Refreshes the tokens, falling back to auth.json when that fails:
    /// another rust-claude may have renewed the tokens since we read it,
    /// spending the refresh token we hold.
    async fn renew(&mut self, http: &reqwest::Client) -> Result<()> {
        match self.refresh(http).await {
            Ok(credentials) => *self = credentials,
            Err(error) => {
                self.adopt_saved();
                if now_millis() >= self.expires {
                    return Err(error);
                }
            }
        }
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

    pub async fn access_token(&mut self, http: &reqwest::Client) -> Result<String> {
        if now_millis() < self.expires {
            return Ok(self.access.clone());
        }
        // Another rust-claude may have renewed the tokens already. Its new
        // refresh token replaces ours, so use what it saved.
        self.adopt_saved();
        if now_millis() >= self.expires {
            self.renew(http)
                .await
                .context("sign-in expired: restart rust-claude to sign in again")?;
        }
        Ok(self.access.clone())
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
    )
    .await?;
    credentials.save()?;
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

async fn request_tokens(http: &reqwest::Client, body: Value) -> Result<Credentials> {
    let response = http.post(token_url().await).json(&body).send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!("token request failed ({status}): {text}");
    }
    let data: Value = serde_json::from_str(&text)?;
    let credentials = Credentials {
        access: data["access_token"]
            .as_str()
            .context("missing access_token")?
            .into(),
        refresh: data["refresh_token"]
            .as_str()
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
    use super::{Credentials, credentials_path, parse_pasted_code};
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
                        let body = body.to_string();
                        let response = format!(
                            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            });
            endpoint
        }
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
