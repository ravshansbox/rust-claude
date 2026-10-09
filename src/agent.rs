use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::{
    auth::Credentials,
    session::{Session, SessionSummary},
    tools,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const SYSTEM_PROMPT: &str = r#"You are rust-claude, a small coding agent running in a terminal.
Use your tools to inspect and change the project in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely."#;
const MAX_TURNS: usize = 20;
const INSTRUCTIONS_FILE: &str = "AGENTS.md";
pub const THINKING_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const DEFAULT_THINKING_LEVEL: &str = "medium";

pub struct Instructions {
    pub label: String,
    text: String,
}

fn load_instructions() -> Vec<Instructions> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if let Some(home) = &home {
        candidates.push((
            format!("~/{INSTRUCTIONS_FILE}"),
            home.join(INSTRUCTIONS_FILE),
        ));
    }
    let same_dir = match (&home, &cwd) {
        (Some(home), Some(cwd)) => same_path(home, cwd),
        _ => false,
    };
    if !same_dir {
        candidates.push((
            format!("./{INSTRUCTIONS_FILE}"),
            PathBuf::from(INSTRUCTIONS_FILE),
        ));
    }

    candidates
        .into_iter()
        .filter_map(|(label, path)| {
            let text = std::fs::read_to_string(path).ok()?;
            (!text.trim().is_empty()).then_some(Instructions { label, text })
        })
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn with_cache_breakpoint(messages: &[Value]) -> Vec<Value> {
    let mut messages = messages.to_vec();
    if let Some(last) = messages.last_mut() {
        if let Some(text) = last["content"].as_str().map(str::to_owned) {
            last["content"] = json!([{ "type": "text", "text": text }]);
        }
        if let Some(block) = last["content"]
            .as_array_mut()
            .and_then(|blocks| blocks.last_mut())
        {
            block["cache_control"] = json!({ "type": "ephemeral" });
        }
    }
    messages
}

#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
    }
}

pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ToolStart { name: String, summary: String },
    ToolDone { name: String, error: Option<String> },
    Usage(Usage),
}

pub struct Agent {
    http: reqwest::Client,
    credentials: Credentials,
    pub model: String,
    pub thinking_level: &'static str,
    messages: Vec<Value>,
    pub instructions: Vec<Instructions>,
    pub session: Session,
}

impl Agent {
    pub fn new(http: reqwest::Client, credentials: Credentials, model: String) -> Result<Self> {
        Ok(Self {
            http,
            credentials,
            model,
            thinking_level: DEFAULT_THINKING_LEVEL,
            messages: Vec::new(),
            instructions: load_instructions(),
            session: Session::new()?,
        })
    }

    pub fn history_len(&self) -> usize {
        self.messages.len()
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.session.list_others()
    }

    pub fn resume(&mut self, id: &str) -> Result<Vec<Value>> {
        let (session, messages) = Session::load(id)?;
        self.session = session;
        self.messages = messages.clone();
        Ok(messages)
    }

    pub fn rollback(&mut self, len: usize) {
        self.messages.truncate(len);
    }

    pub async fn prompt(&mut self, prompt: &str, on_event: impl FnMut(AgentEvent)) -> Result<()> {
        let checkpoint = self.messages.len();
        let result = self.run(prompt, on_event).await;
        if result.is_err() {
            self.messages.truncate(checkpoint);
            return result;
        }
        self.session.save(&self.messages)
    }

    async fn run(&mut self, prompt: &str, mut on_event: impl FnMut(AgentEvent)) -> Result<()> {
        self.messages
            .push(json!({ "role": "user", "content": prompt }));
        let mut total_usage = Usage::default();

        for _ in 0..MAX_TURNS {
            let (content, stop_reason, usage) = self.stream_message(&mut on_event).await?;
            total_usage.add(usage);
            on_event(AgentEvent::Usage(total_usage));
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
    ) -> Result<(Vec<Value>, String, Usage)> {
        let token = self.credentials.access_token(&self.http).await?;
        let mut system = vec![
            json!({ "type": "text", "text": IDENTITY }),
            json!({ "type": "text", "text": SYSTEM_PROMPT }),
        ];
        for instructions in &self.instructions {
            system.push(json!({
                "type": "text",
                "text": format!("# Instructions from {}\n\n{}", instructions.label, instructions.text),
            }));
        }
        if let Some(last) = system.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        let body = json!({
            "model": self.model,
            "max_tokens": 8192,
            "stream": true,
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": self.thinking_level },
            "system": system,
            "tools": tools::definitions(),
            "messages": with_cache_breakpoint(&self.messages),
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
        let mut usage = Usage::default();
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
                        let message_usage = &event["message"]["usage"];
                        usage.input = message_usage["input_tokens"].as_u64().unwrap_or(0);
                        usage.cache_read = message_usage["cache_read_input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                        usage.cache_write = message_usage["cache_creation_input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                    }
                    "content_block_start" => {
                        let block = event["content_block"].clone();
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
                                on_event(AgentEvent::Thinking(thinking.into()));
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
                        usage.output = event["usage"]["output_tokens"].as_u64().unwrap_or(0);
                    }
                    "error" => bail!("{}", event["error"]),
                    _ => {}
                }
            }
        }
        Ok((content, stop_reason, usage))
    }
}
