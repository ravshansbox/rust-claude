use super::{
    Agent, AgentEvent, Usage,
    retry::{is_retryable_status, retry_after, retryable},
    tool_definitions,
};
use crate::models;
use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Instant;

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
        let mut token = self.credentials.access_token(&self.http).await?;
        if let Some(notice) = self.credentials.take_renewal_notice() {
            on_event(AgentEvent::Notice(notice));
        }
        let mut system = self.system_prompt();
        if let Some(last) = system.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": models::max_output(&self.model),
            "stream": true,
            "system": system,
            "tools": tool_definitions(&self.mcp),
            "messages": messages,
        });
        if let (Some(object), Value::Object(settings)) = (
            body.as_object_mut(),
            models::thinking_settings(&self.model, self.thinking_level),
        ) {
            object.extend(settings);
        }
        if let Some(tool_choice) = tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        let mut beta = "oauth-2025-04-20";
        if self.session.drops_mismatched_thinking() {
            body["thinking"]["block_binding"] = json!({ "prefix_mismatch_behavior": "drop_block" });
            beta = "oauth-2025-04-20,thinking-binding-controls-2026-08-01";
        }
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
                        retryable(error, None)
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
        if !response.status().is_success() {
            let status = response.status();
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

        let mut content: Vec<Value> = Vec::new();
        let mut partial_json = String::new();
        let mut input_error = None;
        let mut stop_reason = String::new();
        // Usage of earlier attempts at this request that failed and were
        // retried, so their tokens still count.
        let retried = self.pending_usage;
        let mut usage = Usage::default();
        let mut first_token: Option<Instant> = None;
        let mut buffer: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) if content.is_empty() => return Err(retryable(error, None)),
                Err(error) => return Err(error.into()),
            };
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.windows(2).position(|window| window == b"\n\n") {
                let frame_bytes: Vec<u8> = buffer.drain(..end + 2).collect();
                let frame = String::from_utf8_lossy(&frame_bytes);
                let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
                    continue;
                };
                let event: Value = serde_json::from_str(data)?;
                match event["type"].as_str().unwrap_or_default() {
                    "message_start" => {
                        usage.update_from_response(&event["message"]["usage"]);
                        self.pending_usage = retried;
                        self.pending_usage.add(usage);
                    }
                    "content_block_start" => {
                        first_token.get_or_insert_with(Instant::now);
                        let block = event["content_block"].clone();
                        partial_json.clear();
                        content.push(block);
                    }
                    "content_block_delta" => {
                        let delta = &event["delta"];
                        let Some(block) = content.last_mut() else {
                            continue;
                        };
                        let key = match delta["type"].as_str().unwrap_or_default() {
                            "text_delta" => "text",
                            "thinking_delta" => "thinking",
                            "signature_delta" => "signature",
                            "input_json_delta" => {
                                partial_json
                                    .push_str(delta["partial_json"].as_str().unwrap_or_default());
                                continue;
                            }
                            _ => continue,
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
                    "content_block_stop" => {
                        if let Some(block) = content.last_mut()
                            && block["type"] == "tool_use"
                            && !partial_json.is_empty()
                        {
                            match serde_json::from_str(&partial_json) {
                                Ok(input) => block["input"] = input,
                                Err(error) => input_error = Some(error),
                            }
                        }
                    }
                    "message_delta" => {
                        stop_reason = event["delta"]["stop_reason"]
                            .as_str()
                            .unwrap_or_default()
                            .into();
                        usage.update_from_response(&event["usage"]);
                        usage.duration_ms =
                            first_token.map_or(0, |start| start.elapsed().as_millis() as u64);
                        self.pending_usage = retried;
                        self.pending_usage.add(usage);
                    }
                    "error" => {
                        let message = event["error"].to_string();
                        let retry = matches!(
                            event["error"]["type"].as_str(),
                            Some("overloaded_error" | "api_error" | "rate_limit_error")
                        );
                        if retry && content.is_empty() {
                            return Err(retryable(message, None));
                        }
                        bail!(message);
                    }
                    _ => {}
                }
            }
        }
        if stop_reason.is_empty() {
            bail!("response ended before the reply finished");
        }
        if stop_reason != "tool_use" && content.iter().any(|block| block["type"] == "tool_use") {
            bail!("reply stopped ({stop_reason}) before its tool call finished");
        }
        if let Some(error) = input_error {
            return Err(error.into());
        }
        Ok(Response {
            content,
            stop_reason,
            usage,
            retried,
        })
    }
}
