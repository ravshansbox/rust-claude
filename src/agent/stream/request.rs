use super::retryable_transport;
use crate::agent::{
    Agent, AgentEvent, OAUTH_BETA,
    retry::{is_retryable_status, retry_after, retryable},
};
use crate::models;
use anyhow::{Result, bail};
use serde_json::{Value, json};

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
            OAUTH_BETA
        };
        (body, beta)
    }

    pub(super) async fn send_request(
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
}
