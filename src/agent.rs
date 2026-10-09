use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::{
    auth::Credentials,
    models,
    session::{Session, SessionSummary},
    tools,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1000";
const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const SYSTEM_PROMPT: &str = r#"You are rust-claude, a small coding agent running in a terminal.
Use your tools to inspect and change the project in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely."#;
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
    let mut messages: Vec<Value> = messages
        .iter()
        .filter(|message| message.get("stop_reason").is_none())
        .cloned()
        .collect();
    for message in &mut messages {
        if let Some(object) = message.as_object_mut() {
            object.remove("usage");
        }
    }
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
    pub cost: f64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.cost += other.cost;
    }

    fn priced(mut self, model: &str) -> Self {
        self.cost = models::cost(
            model,
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
        );
        self
    }

    fn to_json(self) -> Value {
        json!({
            "input": self.input,
            "output": self.output,
            "cache_read": self.cache_read,
            "cache_write": self.cache_write,
            "cost": self.cost,
        })
    }

    fn from_json(value: &Value) -> Self {
        Self {
            input: value["input"].as_u64().unwrap_or(0),
            output: value["output"].as_u64().unwrap_or(0),
            cache_read: value["cache_read"].as_u64().unwrap_or(0),
            cache_write: value["cache_write"].as_u64().unwrap_or(0),
            cost: value["cost"].as_f64().unwrap_or(0.0),
        }
    }
}

pub struct Stats {
    pub usage: Usage,
    pub cache_hit_rate: Option<f64>,
    pub context_tokens: u64,
    pub context_window: u64,
}

fn estimate_tokens(message: &Value) -> u64 {
    let characters: usize = match &message["content"] {
        Value::String(text) => text.chars().count(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block["type"].as_str().unwrap_or_default() {
                "text" => block["text"].as_str().unwrap_or_default().chars().count(),
                "thinking" => block["thinking"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .count(),
                "tool_use" => {
                    block["name"].as_str().unwrap_or_default().chars().count()
                        + block["input"].to_string().chars().count()
                }
                "tool_result" => block["content"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .count(),
                _ => 0,
            })
            .sum(),
        _ => 0,
    };
    (characters as u64).div_ceil(4)
}

fn context_tokens(messages: &[Value]) -> u64 {
    let last_usage = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            if message["role"] != "assistant" || message.get("stop_reason").is_some() {
                return None;
            }
            let usage = Usage::from_json(&message["usage"]);
            let tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
            (tokens > 0).then_some((index, tokens))
        });
    let (start, usage_tokens) = last_usage.map_or((0, 0), |(index, tokens)| (index + 1, tokens));
    usage_tokens + messages[start..].iter().map(estimate_tokens).sum::<u64>()
}

fn total_usage(messages: &[Value]) -> Usage {
    let mut total = Usage::default();
    for message in messages
        .iter()
        .filter(|message| message["role"] == "assistant")
    {
        total.add(Usage::from_json(&message["usage"]));
    }
    total
}

pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ToolStart { name: String, summary: String },
    ToolDone { name: String, error: Option<String> },
    Stats(Stats),
    Notice(String),
}

pub struct Agent {
    http: reqwest::Client,
    credentials: Credentials,
    pub model: String,
    pub thinking_level: &'static str,
    messages: Vec<Value>,
    pending_usage: Usage,
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
            pending_usage: Usage::default(),
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

    pub async fn list_models(&mut self) -> Result<Vec<String>> {
        let token = self.credentials.access_token(&self.http).await?;
        let response = self
            .http
            .get(MODELS_URL)
            .bearer_auth(token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            bail!("{status}: {}", response.text().await?);
        }
        let body: Value = response.json().await?;
        Ok(body["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model["id"].as_str().map(str::to_string))
            .collect())
    }

    pub fn new_session(&mut self) -> Result<()> {
        self.session = Session::new()?;
        self.messages.clear();
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        let cache_hit_rate = self
            .messages
            .iter()
            .rfind(|message| message["role"] == "assistant")
            .map(|message| Usage::from_json(&message["usage"]))
            .and_then(|usage| {
                let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
                (prompt_tokens > 0).then(|| usage.cache_read as f64 / prompt_tokens as f64 * 100.0)
            });
        Stats {
            usage: total_usage(&self.messages),
            cache_hit_rate,
            context_tokens: context_tokens(&self.messages),
            context_window: models::context_window(&self.model),
        }
    }

    fn discard_from(&mut self, index: usize, stop_reason: &str) {
        let mut lost = total_usage(&self.messages[index..]);
        lost.add(std::mem::take(&mut self.pending_usage).priced(&self.model));
        self.messages.truncate(index);
        if lost.input + lost.output + lost.cache_read + lost.cache_write > 0 {
            self.messages.push(json!({
                "role": "assistant",
                "stop_reason": stop_reason,
                "content": [],
                "usage": lost.to_json(),
            }));
        }
    }

    pub fn cancel(&mut self, checkpoint: usize) -> Result<()> {
        let finished = self.messages[checkpoint..]
            .iter()
            .rposition(|message| message["role"] == "user" && message["content"].is_array());
        match finished {
            Some(index) => self.discard_from(checkpoint + index + 1, "aborted"),
            None => self.discard_from(checkpoint, "aborted"),
        }
        self.session.save(&self.messages)
    }

    pub async fn prompt(&mut self, prompt: &str, on_event: impl FnMut(AgentEvent)) -> Result<()> {
        let checkpoint = self.messages.len();
        let result = self.run(prompt, on_event).await;
        if result.is_err() {
            self.discard_from(checkpoint, "error");
            self.session.save(&self.messages)?;
            return result;
        }
        self.session.save(&self.messages)
    }

    async fn run(&mut self, prompt: &str, mut on_event: impl FnMut(AgentEvent)) -> Result<()> {
        self.messages
            .push(json!({ "role": "user", "content": prompt }));
        on_event(AgentEvent::Stats(self.stats()));

        loop {
            let (content, stop_reason, usage) = self.stream_message(&mut on_event).await?;
            self.messages
                .push(json!({ "role": "assistant", "content": content, "usage": usage.priced(&self.model).to_json() }));
            self.pending_usage = Usage::default();
            on_event(AgentEvent::Stats(self.stats()));
            if stop_reason == "max_tokens" {
                on_event(AgentEvent::Notice(
                    "reply cut off: max_tokens reached".into(),
                ));
            }
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
        let mut buffer: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk?);
            while let Some(end) = buffer.windows(2).position(|window| window == b"\n\n") {
                let frame_bytes: Vec<u8> = buffer.drain(..end + 2).collect();
                let frame = String::from_utf8_lossy(&frame_bytes);
                let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) else {
                    continue;
                };
                let event: Value = serde_json::from_str(data)?;
                match event["type"].as_str().unwrap_or_default() {
                    "message_start" => {
                        let message_usage = &event["message"]["usage"];
                        usage.input = message_usage["input_tokens"].as_u64().unwrap_or(0);
                        usage.output = message_usage["output_tokens"].as_u64().unwrap_or(0);
                        usage.cache_read = message_usage["cache_read_input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                        usage.cache_write = message_usage["cache_creation_input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                        self.pending_usage = usage;
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
                        let delta_usage = &event["usage"];
                        if let Some(input) = delta_usage["input_tokens"].as_u64() {
                            usage.input = input;
                        }
                        if let Some(output) = delta_usage["output_tokens"].as_u64() {
                            usage.output = output;
                        }
                        if let Some(cache_read) = delta_usage["cache_read_input_tokens"].as_u64() {
                            usage.cache_read = cache_read;
                        }
                        if let Some(cache_write) =
                            delta_usage["cache_creation_input_tokens"].as_u64()
                        {
                            usage.cache_write = cache_write;
                        }
                        self.pending_usage = usage;
                    }
                    "error" => bail!("{}", event["error"]),
                    _ => {}
                }
            }
        }
        Ok((content, stop_reason, usage))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::context_tokens;

    #[test]
    fn counts_context_from_last_usage_and_trailing_messages() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hi" }],
                "usage": { "input": 10, "output": 5, "cache_read": 100, "cache_write": 20 },
            }),
            json!({
                "role": "assistant",
                "stop_reason": "aborted",
                "content": [],
                "usage": { "input": 999 },
            }),
            json!({ "role": "user", "content": "12345678" }),
        ];
        assert_eq!(context_tokens(&messages), 137);
    }

    #[test]
    fn estimates_context_without_usage() {
        let messages = vec![json!({ "role": "user", "content": "12345" })];
        assert_eq!(context_tokens(&messages), 2);
    }
}
