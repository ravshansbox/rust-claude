use super::{Agent, AgentEvent, Usage, retry::retryable};
use anyhow::{Result, bail};
use serde_json::Value;
use std::time::Instant;

mod blocks;
mod request;

use blocks::{apply_delta, finish_block, start_block};

fn retryable_transport(error: reqwest::Error) -> anyhow::Error {
    retryable(format!("{:#}", anyhow::Error::from(error)), None)
}

/// A finished reply and the tokens it used.
pub(super) struct Response {
    pub content: Vec<Value>,
    pub stop_reason: String,
    /// Usage of the attempt that produced this reply.
    pub usage: Usage,
    /// Usage of earlier attempts at this request that failed and were retried.
    pub retried: Usage,
}

impl Agent {
    pub(super) async fn stream_message(
        &mut self,
        messages: &[Value],
        tool_choice: Option<&Value>,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Response> {
        let mut response = self.send_request(messages, tool_choice, on_event).await?;
        let mut content = Vec::new();
        let mut partial_json = String::new();
        let mut input_error = None;
        let mut stop_reason = String::new();
        // Usage of earlier attempts at this request that failed and were
        // retried, so their tokens still count.
        let retried = self.pending_usage;
        let mut usage = Usage::default();
        let mut first_token: Option<Instant> = None;
        let mut buffer = Vec::new();

        while let Some(chunk) = response.chunk().await.transpose() {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) if content.is_empty() => {
                    return Err(retryable_transport(error));
                }
                Err(error) => return Err(error.into()),
            };
            buffer.extend_from_slice(&chunk);
            let mut start = 0;
            while let Some(end) = buffer[start..]
                .windows(2)
                .position(|window| window == b"\n\n")
            {
                let frame = String::from_utf8_lossy(&buffer[start..start + end + 2]);
                start += end + 2;
                let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
                    continue;
                };
                let mut event: Value = serde_json::from_str(data)?;
                match event["type"].as_str().unwrap_or_default() {
                    "message_start" => usage.update_from_response(&event["message"]["usage"]),
                    "content_block_start" => {
                        first_token.get_or_insert_with(Instant::now);
                        partial_json.clear();
                        start_block(&mut content, event["content_block"].take(), on_event);
                    }
                    "content_block_delta" => {
                        apply_delta(
                            &mut content,
                            &mut partial_json,
                            &mut event["delta"],
                            on_event,
                        );
                    }
                    "content_block_stop" => {
                        finish_block(&mut content, &partial_json, &mut input_error, on_event);
                    }
                    "message_delta" => {
                        stop_reason = finish_message(&event, &mut usage, first_token);
                    }
                    "error" => return Err(stream_error(&event["error"], content.is_empty())),
                    _ => {}
                }
                self.pending_usage = retried;
                self.pending_usage.add(usage);
            }
            buffer.drain(..start);
        }
        check_finished(&stop_reason, &content, input_error)?;
        Ok(Response {
            content,
            stop_reason,
            usage,
            retried,
        })
    }
}

fn finish_message(event: &Value, usage: &mut Usage, first_token: Option<Instant>) -> String {
    usage.update_from_response(&event["usage"]);
    usage.duration_ms = first_token.map_or(0, |start| {
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    });
    event["delta"]["stop_reason"]
        .as_str()
        .unwrap_or_default()
        .into()
}

fn stream_error(error: &Value, nothing_received: bool) -> anyhow::Error {
    let message = error.to_string();
    let retry = matches!(
        error["type"].as_str(),
        Some("overloaded_error" | "api_error" | "rate_limit_error")
    );
    if retry && nothing_received {
        return retryable(message, None);
    }
    anyhow::anyhow!(message)
}

fn check_finished(
    stop_reason: &str,
    content: &[Value],
    input_error: Option<serde_json::Error>,
) -> Result<()> {
    if stop_reason.is_empty() {
        bail!("response ended before the reply finished");
    }
    // A refusal is reported as such by the caller, which drops the reply
    // along with any tool call it left unfinished.
    if stop_reason != "refusal" {
        if stop_reason != "tool_use" && content.iter().any(|block| block["type"] == "tool_use") {
            bail!("reply stopped ({stop_reason}) before its tool call finished");
        }
        if let Some(error) = input_error {
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
