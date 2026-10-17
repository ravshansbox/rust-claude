use serde_json::Value;

/// Reads a `key="value"` or `key=value` parameter of a `WWW-Authenticate`
/// challenge, skipping text inside quoted values.
pub(super) fn challenge_parameter(challenge: &str, key: &str) -> Option<String> {
    let mut quoted = false;
    let mut escaped = false;
    let mut before = None;
    for (start, character) in challenge.char_indices() {
        let at_key = !quoted
            && !before.is_some_and(|before: char| {
                before.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(before)
            });
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
            Some(quoted) => unquote(quoted),
            None => value
                .split([',', ' '])
                .next()
                .unwrap_or_default()
                .to_string(),
        });
    }
    None
}

/// The text of a quoted value up to its closing quote, with each `\`
/// escape replaced by the character it escapes.
fn unquote(quoted: &str) -> String {
    let mut value = String::new();
    let mut characters = quoted.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => break,
            '\\' => value.extend(characters.next()),
            _ => value.push(character),
        }
    }
    value
}

pub(super) fn requested_scope(
    challenge: &str,
    resource_metadata: Option<&Value>,
) -> Option<String> {
    challenge_parameter(challenge, "scope").or_else(|| {
        let scopes: Vec<&str> = resource_metadata?
            .get("scopes_supported")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        (!scopes.is_empty()).then(|| scopes.join(" "))
    })
}

/// The `WWW-Authenticate` header the server sends to an unsigned `initialize` request.
pub(super) async fn challenge(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<String, String> {
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
            "params": crate::mcp::initialize_params(),
        }))
        .send()
        .await
        .map_err(|error| format!("cannot reach {server_url}: {error}"))?;
    Ok(probe
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", "))
}
