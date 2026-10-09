use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::{auth::Credentials, tools};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const SYSTEM_PROMPT: &str = r#"You are rust-claude, a small coding agent running in a terminal.
Use your tools to inspect and change the project in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely."#;
const MAX_TURNS: usize = 20;

pub enum AgentEvent {
    Text(String),
    ToolCall(String),
    ToolStart { name: String, summary: String },
    ToolDone { name: String, error: Option<String> },
    Usage { input: u64, output: u64 },
}

pub struct Agent {
    http: reqwest::Client,
    credentials: Credentials,
    pub model: String,
    messages: Vec<Value>,
}

impl Agent {
    pub fn new(http: reqwest::Client, credentials: Credentials, model: String) -> Self {
        Self {
            http,
            credentials,
            model,
            messages: Vec::new(),
        }
    }

    pub async fn prompt(&mut self, prompt: &str, on_event: impl FnMut(AgentEvent)) -> Result<()> {
        let checkpoint = self.messages.len();
        let result = self.run(prompt, on_event).await;
        if result.is_err() {
            self.messages.truncate(checkpoint);
        }
        result
    }

    async fn run(&mut self, prompt: &str, mut on_event: impl FnMut(AgentEvent)) -> Result<()> {
        self.messages
            .push(json!({ "role": "user", "content": prompt }));
        let mut input_tokens = 0;
        let mut output_tokens = 0;

        for _ in 0..MAX_TURNS {
            let (content, stop_reason, usage) = self.stream_message(&mut on_event).await?;
            input_tokens += usage.0;
            output_tokens += usage.1;
            on_event(AgentEvent::Usage {
                input: input_tokens,
                output: output_tokens,
            });
            self.messages
                .push(json!({ "role": "assistant", "content": content }));
            if stop_reason != "tool_use" {
                return Ok(());
            }

            let mut results = Vec::new();
            for block in content.iter().filter(|block| block["type"] == "tool_use") {
                let name = block["name"].as_str().unwrap_or_default();
                on_event(AgentEvent::ToolStart {
                    name: name.into(),
                    summary: tools::summary(name, &block["input"]),
                });
                let (text, is_error) = match tools::call(name, &block["input"]).await {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                on_event(AgentEvent::ToolDone {
                    name: name.into(),
                    error: is_error.then(|| text.clone()),
                });
                results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": block["id"],
                    "content": text,
                    "is_error": is_error,
                }));
            }
            self.messages
                .push(json!({ "role": "user", "content": results }));
        }
        bail!("stopped after {MAX_TURNS} turns")
    }

    async fn stream_message(
        &mut self,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<(Vec<Value>, String, (u64, u64))> {
        let token = self.credentials.access_token(&self.http).await?;
        let body = json!({
            "model": self.model,
            "max_tokens": 8192,
            "stream": true,
            "system": [
                { "type": "text", "text": IDENTITY },
                { "type": "text", "text": SYSTEM_PROMPT },
            ],
            "tools": tools::definitions(),
            "messages": self.messages,
        });
        let response = self
            .http
            .post(API_URL)
            .bearer_auth(token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20")
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            bail!("{status}: {}", response.text().await?);
        }

        let mut content: Vec<Value> = Vec::new();
        let mut partial_json = String::new();
        let mut stop_reason = String::new();
        let mut usage = (0, 0);
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(end) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..end + 2).collect();
                let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
                    continue;
                };
                let event: Value = serde_json::from_str(data)?;
                match event["type"].as_str().unwrap_or_default() {
                    "message_start" => {
                        usage.0 = event["message"]["usage"]["input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                    }
                    "content_block_start" => {
                        let block = event["content_block"].clone();
                        if block["type"] == "tool_use" {
                            on_event(AgentEvent::ToolCall(
                                block["name"].as_str().unwrap_or_default().into(),
                            ));
                        }
                        partial_json.clear();
                        content.push(block);
                    }
                    "content_block_delta" => {
                        let delta = &event["delta"];
                        let Some(block) = content.last_mut() else {
                            continue;
                        };
                        match delta["type"].as_str().unwrap_or_default() {
                            "text_delta" => {
                                let text = delta["text"].as_str().unwrap_or_default();
                                let previous = block["text"].as_str().unwrap_or_default();
                                block["text"] = json!(format!("{previous}{text}"));
                                on_event(AgentEvent::Text(text.into()));
                            }
                            "thinking_delta" => {
                                let thinking = delta["thinking"].as_str().unwrap_or_default();
                                let previous = block["thinking"].as_str().unwrap_or_default();
                                block["thinking"] = json!(format!("{previous}{thinking}"));
                            }
                            "signature_delta" => {
                                let signature = delta["signature"].as_str().unwrap_or_default();
                                let previous = block["signature"].as_str().unwrap_or_default();
                                block["signature"] = json!(format!("{previous}{signature}"));
                            }
                            "input_json_delta" => {
                                partial_json
                                    .push_str(delta["partial_json"].as_str().unwrap_or_default());
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        if let Some(block) = content.last_mut()
                            && block["type"] == "tool_use"
                            && !partial_json.is_empty()
                        {
                            block["input"] = serde_json::from_str(&partial_json)?;
                        }
                    }
                    "message_delta" => {
                        stop_reason = event["delta"]["stop_reason"]
                            .as_str()
                            .unwrap_or_default()
                            .into();
                        usage.1 = event["usage"]["output_tokens"].as_u64().unwrap_or(0);
                    }
                    "error" => bail!("{}", event["error"]),
                    _ => {}
                }
            }
        }
        Ok((content, stop_reason, usage))
    }
}
