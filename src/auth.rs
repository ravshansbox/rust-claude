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
}

fn credentials_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".rust-claude").join("auth.json"))
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
                println!("Saved sign-in is not usable ({error}). Sign in again.\n");
                login(http).await
            }
        }
    }

    async fn load_and_refresh(text: &str, http: &reqwest::Client) -> Result<Self> {
        let credentials: Self = serde_json::from_str(text)?;
        if now_millis() < credentials.expires {
            return Ok(credentials);
        }
        credentials.refresh(http).await
    }

    async fn refresh(&self, http: &reqwest::Client) -> Result<Self> {
        request_tokens(
            http,
            json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": self.refresh,
            }),
        )
        .await
    }

    pub async fn access_token(&mut self, http: &reqwest::Client) -> Result<(String, bool)> {
        let renewed = now_millis() >= self.expires;
        if renewed {
            *self = self
                .refresh(http)
                .await
                .context("sign-in expired: restart rust-claude to sign in again")?;
        }
        Ok((self.access.clone(), renewed))
    }

    fn save(&self) -> Result<()> {
        let path = credentials_path()?;
        std::fs::create_dir_all(path.parent().context("invalid credentials path")?)?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

async fn login(http: &reqwest::Client) -> Result<Credentials> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|error| anyhow::anyhow!("{error}"))?;
    let verifier = URL_SAFE_NO_PAD.encode(random);
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
            ("state", verifier.as_str()),
        ],
    )?;

    println!("Open this URL and sign in with your Claude Pro/Max account:\n\n{url}\n");
    print!("Paste the code: ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    let (code, state) = match input.trim().split_once('#') {
        Some((code, state)) => (code.to_string(), state.to_string()),
        None => (input.trim().to_string(), verifier.clone()),
    };
    if state != verifier {
        bail!("OAuth state mismatch");
    }

    request_tokens(
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
    .await
}

async fn request_tokens(http: &reqwest::Client, body: Value) -> Result<Credentials> {
    let response = http.post(TOKEN_URL).json(&body).send().await?;
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
    };
    credentials.save()?;
    Ok(credentials)
}
