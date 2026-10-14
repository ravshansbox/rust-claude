use super::{
    Agent, AgentEvent, Usage,
    retry::{is_retryable_status, retry_after, retryable},
};
use crate::{models, tools};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::time::Instant;

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
    fn request_body(
        &self,
        messages: &[Value],
        tool_choice: Option<&Value>,
    ) -> (Value, &'static str) {
        let mut system = self.system_prompt();
        if let Some(last) = system.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": models::max_output(&self.model),
            "stream": true,
            "system": system,
            "tools": self.tool_definitions(),
            "messages": messages,
        });
        if let (Some(object), Value::Object(settings)) = (
            body.as_object_mut(),
            models::thinking_settings(&self.model, self.effort),
        ) {
            object.extend(settings);
        }
        if let Some(tool_choice) = tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        let beta = if self.session.drops_mismatched_thinking()
            && body["thinking"]["type"] == "adaptive"
        {
            body["thinking"]["block_binding"] = json!({ "prefix_mismatch_behavior": "drop_block" });
            "oauth-2025-04-20,thinking-binding-controls-2026-08-01"
        } else {
            "oauth-2025-04-20"
        };
        (body, beta)
    }

    async fn send_request(
        &mut self,
        messages: &[Value],
        tool_choice: Option<&Value>,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<reqwest::Response> {
        let mut token = self.credentials.access_token(&self.http).await?;
        if let Some(notice) = self.credentials.take_renewal_notice() {
            on_event(AgentEvent::Notice(notice));
        }
        let (body, beta) = self.request_body(messages, tool_choice);
        // The API may turn down a token before it expires, as when the
        // sign-in was revoked; renewing it is then worth one more try.
        let mut renewed = false;
        let response = loop {
            let response = self
                .http
                .post(format!("{}/v1/messages", self.api_base))
                .bearer_auth(&token)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", beta)
                .json(&body)
                .send()
                .await
                .map_err(|error| {
                    if error.is_connect() || error.is_timeout() || error.is_request() {
                        retryable_transport(error)
                    } else {
                        error.into()
                    }
                })?;
            if response.status() != reqwest::StatusCode::UNAUTHORIZED || renewed {
                break response;
            }
            token = self.credentials.renew_rejected(&self.http, &token).await?;
            renewed = true;
            if let Some(notice) = self.credentials.take_renewal_notice() {
                on_event(AgentEvent::Notice(notice));
            }
        };
        self.quota.update_from_headers(response.headers());
        let status = response.status();
        if !status.is_success() {
            let delay = retry_after(response.headers());
            // A body that breaks off still leaves the status to go by.
            let text = response
                .text()
                .await
                .unwrap_or_else(|error| error.to_string());
            let message = format!("{status}: {text}");
            if is_retryable_status(status) {
                return Err(retryable(message, delay));
            }
            bail!(message);
        }
        Ok(response)
    }

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
                        stop_reason = event["delta"]["stop_reason"]
                            .as_str()
                            .unwrap_or_default()
                            .into();
                        usage.update_from_response(&event["usage"]);
                        usage.duration_ms = first_token.map_or(0, |start| {
                            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
                        });
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

fn start_block(content: &mut Vec<Value>, block: Value, on_event: &mut impl FnMut(AgentEvent)) {
    if block["type"] == "web_search_tool_result" {
        let outcome = tools::web_search::outcome(&block["content"]);
        on_event(AgentEvent::ToolDone {
            name: tools::web_search::NAME.into(),
            note: outcome.as_ref().ok().cloned(),
            error: outcome.err(),
        });
    }
    content.push(block);
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

fn apply_delta(
    content: &mut [Value],
    partial_json: &mut String,
    delta: &mut Value,
    on_event: &mut impl FnMut(AgentEvent),
) {
    let Some(block) = content.last_mut() else {
        return;
    };
    let key = match delta["type"].as_str().unwrap_or_default() {
        "text_delta" => "text",
        "thinking_delta" => "thinking",
        "signature_delta" => "signature",
        "citations_delta" => {
            let citation = delta["citation"].take();
            match block.get_mut("citations") {
                Some(Value::Array(citations)) => citations.push(citation),
                _ => block["citations"] = Value::Array(vec![citation]),
            }
            return;
        }
        "input_json_delta" => {
            partial_json.push_str(delta["partial_json"].as_str().unwrap_or_default());
            return;
        }
        _ => return,
    };
    let addition = delta[key].as_str().unwrap_or_default();
    match block.get_mut(key) {
        Some(Value::String(text)) => text.push_str(addition),
        _ => block[key] = json!(addition),
    }
    match key {
        "text" => on_event(AgentEvent::Text(addition.into())),
        "thinking" => on_event(AgentEvent::Thinking(addition.into())),
        _ => {}
    }
}

fn finish_block(
    content: &mut [Value],
    partial_json: &str,
    input_error: &mut Option<serde_json::Error>,
    on_event: &mut impl FnMut(AgentEvent),
) {
    if let Some(block) = content.last_mut()
        && (block["type"] == "tool_use" || block["type"] == "server_tool_use")
        && !partial_json.is_empty()
    {
        match serde_json::from_str(partial_json) {
            Ok(input) => block["input"] = input,
            Err(error) => *input_error = Some(error),
        }
    }
    if let Some(block) = content.last()
        && block["type"] == "server_tool_use"
    {
        let name = block["name"].as_str().unwrap_or_default();
        on_event(AgentEvent::ToolStart {
            name: name.into(),
            summary: tools::summary(name, &block["input"]),
            diff: None,
        });
    }
}

#[cfg(test)]
mod tests;
