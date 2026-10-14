use crate::auth::expires_at;
use serde_json::Value;

mod callback;
mod discovery;
mod saved;
mod sign_in;

pub(super) use saved::{Saved, load, load_for, remove, save};
pub use sign_in::Pending;
pub(super) use sign_in::begin;

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
