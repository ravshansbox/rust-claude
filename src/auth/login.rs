use std::io::{self, Write};

use anyhow::{Result, bail};
use serde_json::json;

use super::{CLIENT_ID, Credentials, pkce_challenge, random_token, request_tokens};

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

pub(super) async fn login(http: &reqwest::Client) -> Result<Credentials> {
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
pub(super) async fn exchange_code(
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

/// Splits the `code#state` text the callback page shows and checks the
/// state against the one we sent. The page always includes the state, so
/// text without it was not copied whole.
pub(super) fn parse_pasted_code<'a>(input: &'a str, expected_state: &str) -> Result<&'a str> {
    let Some((code, state)) = input.trim().split_once('#') else {
        bail!("the pasted code has no #state part; paste the whole code shown after sign-in");
    };
    if state != expected_state {
        bail!("OAuth state mismatch");
    }
    Ok(code)
}
