use serde_json::Value;

use crate::mcp::MAX_LINE;

fn too_long() -> String {
    format!("server sent a reply longer than {} MiB", MAX_LINE >> 20)
}

fn read_error(method: &str, error: reqwest::Error) -> String {
    format!(
        "cannot read the reply to {method}: {:#}",
        anyhow::Error::from(error)
    )
}

pub(super) async fn read_json(
    mut response: reqwest::Response,
    method: &str,
    mut handle: impl FnMut(&Value) -> bool,
) -> Result<(), String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| read_error(method, error))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_LINE {
            return Err(too_long());
        }
    }
    let body: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("server sent invalid JSON: {error}"))?;
    let replies = match &body {
        Value::Array(replies) => replies.as_slice(),
        reply => std::slice::from_ref(reply),
    };
    for reply in replies {
        handle(reply);
    }
    Ok(())
}

pub(super) async fn read_event_stream(
    mut response: reqwest::Response,
    method: &str,
    mut handle: impl FnMut(&Value) -> bool,
) -> Result<(), String> {
    let mut buffer = Vec::new();
    let mut data = Vec::new();
    let mut is_message = true;
    let mut after_cr = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| read_error(method, error))?
    {
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer
            .iter()
            .position(|&byte| byte == b'\n' || byte == b'\r')
        {
            let mut line: Vec<u8> = buffer.drain(..=end).collect();
            let ending = line.pop();
            if after_cr && line.is_empty() && ending == Some(b'\n') {
                after_cr = false;
                continue;
            }
            after_cr = ending == Some(b'\r');
            if line.is_empty() {
                if is_message
                    && let Ok(reply) = serde_json::from_slice::<Value>(&data)
                    && handle(&reply)
                {
                    return Ok(());
                }
                data.clear();
                is_message = true;
                continue;
            }
            let (field, value) = match line.iter().position(|&byte| byte == b':') {
                Some(colon) => (&line[..colon], &line[colon + 1..]),
                None => (&line[..], &[][..]),
            };
            let value = value.strip_prefix(b" ").unwrap_or(value);
            match field {
                b"data" => {
                    if !data.is_empty() {
                        data.push(b'\n');
                    }
                    data.extend_from_slice(value);
                }
                b"event" => is_message = matches!(value, b"" | b"message"),
                _ => {}
            }
            if data.len() > MAX_LINE {
                return Err(too_long());
            }
        }
        if buffer.len() > MAX_LINE {
            return Err(too_long());
        }
    }
    Ok(())
}
