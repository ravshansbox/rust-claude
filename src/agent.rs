use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::{
    ask::{self, Question},
    auth::Credentials,
    images::Image,
    mcp::Mcp,
    models,
    session::{Session, SessionSummary},
    skills::{self, Skills},
    tools,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1000";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const SYSTEM_PROMPT: &str = r#"You are rust-claude, a small coding agent running in a terminal.
Use your tools to inspect and change the project in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely.
Prefer edit and write over bash for changing files.
Search code with ast-grep. Fall back to ripgrep for plain text, comments, strings and files ast-grep cannot parse.
Put questions to the user in bold."#;
const INSTRUCTIONS_FILE: &str = "AGENTS.md";
pub const THINKING_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const DEFAULT_THINKING_LEVEL: &str = "medium";
const COMPACT_PROMPT: &str = "Summarise this conversation so that you can continue the work from the summary alone. Include the user's requests, decisions made, files read and changed, the current state of the work and the next steps. Do not call tools. Reply with the summary only.";
const SUMMARY_INTRODUCTION: &str = "The earlier conversation was compacted to save context. You wrote the summary below of everything that happened in it. Treat it as an accurate record and continue from where the conversation stopped.";
const COMPACT_AT_PERCENT: u64 = 80;
const MAX_RETRIES: u32 = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Retryable {
    message: String,
    retry_after: Option<Duration>,
}

impl std::fmt::Display for Retryable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Retryable {}

fn retryable(message: impl ToString, retry_after: Option<Duration>) -> anyhow::Error {
    Retryable {
        message: message.to_string(),
        retry_after,
    }
    .into()
}

fn retry_delay(error: &anyhow::Error, attempt: u32) -> Option<Duration> {
    let error = error.downcast_ref::<Retryable>()?;
    if attempt >= MAX_RETRIES {
        return None;
    }
    match error.retry_after {
        Some(delay) if delay > MAX_RETRY_AFTER => None,
        Some(delay) => Some(delay),
        None => Some(Duration::from_secs(1 << attempt)),
    }
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.as_u16() == 529
        || status.is_server_error()
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

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

fn is_compaction(message: &Value) -> bool {
    message["stop_reason"] == "compacted"
}

fn active_messages(messages: &[Value]) -> Vec<Value> {
    let Some(index) = messages.iter().rposition(is_compaction) else {
        return messages.to_vec();
    };
    let summary = messages[index]["summary"].as_str().unwrap_or_default();
    let mut active = vec![json!({
        "role": "user",
        "content": format!("{SUMMARY_INTRODUCTION}\n\n{summary}"),
    })];
    active.extend_from_slice(&messages[index + 1..]);
    active
}

fn has_uncompacted(messages: &[Value]) -> bool {
    let start = messages
        .iter()
        .rposition(is_compaction)
        .map_or(0, |index| index + 1);
    messages[start..]
        .iter()
        .any(|message| message.get("stop_reason").is_none())
}

fn with_cache_breakpoint(messages: &[Value]) -> Vec<Value> {
    let mut messages: Vec<Value> = messages
        .iter()
        .filter(|message| {
            message.get("stop_reason").is_none()
                && message["content"]
                    .as_array()
                    .is_none_or(|blocks| !blocks.is_empty())
        })
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
    pub duration_ms: u64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.duration_ms += other.duration_ms;
    }

    fn to_json(self) -> Value {
        json!({
            "input": self.input,
            "output": self.output,
            "cache_read": self.cache_read,
            "cache_write": self.cache_write,
            "duration_ms": self.duration_ms,
        })
    }

    fn update_from_response(&mut self, value: &Value) {
        let fields = [
            (&mut self.input, "input_tokens"),
            (&mut self.output, "output_tokens"),
            (&mut self.cache_read, "cache_read_input_tokens"),
            (&mut self.cache_write, "cache_creation_input_tokens"),
        ];
        for (field, key) in fields {
            if let Some(count) = value[key].as_u64() {
                *field = count;
            }
        }
    }

    fn from_json(value: &Value) -> Self {
        Self {
            input: value["input"].as_u64().unwrap_or(0),
            output: value["output"].as_u64().unwrap_or(0),
            cache_read: value["cache_read"].as_u64().unwrap_or(0),
            cache_write: value["cache_write"].as_u64().unwrap_or(0),
            duration_ms: value["duration_ms"].as_u64().unwrap_or(0),
        }
    }
}

pub struct Stats {
    pub usage: Usage,
    pub cache_hit_rate: Option<f64>,
    pub tokens_per_second: Option<f64>,
    pub context_tokens: u64,
    pub context_window: u64,
    pub quota: Quota,
}

#[derive(Default, Clone, Copy)]
pub struct Quota {
    pub five_hour_remaining: Option<f64>,
    pub seven_day_remaining: Option<f64>,
    pub five_hour_reset: Option<u64>,
    pub seven_day_reset: Option<u64>,
}

impl Quota {
    fn update_from_headers(&mut self, headers: &reqwest::header::HeaderMap) {
        let remaining = |window: &str| {
            headers
                .get(format!("anthropic-ratelimit-unified-{window}-utilization"))
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<f64>().ok())
                .map(|utilisation| (1.0 - utilisation) * 100.0)
        };
        let reset = |window: &str| {
            headers
                .get(format!("anthropic-ratelimit-unified-{window}-reset"))
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
        };
        self.five_hour_remaining = remaining("5h").or(self.five_hour_remaining);
        self.seven_day_remaining = remaining("7d").or(self.seven_day_remaining);
        self.five_hour_reset = reset("5h").or(self.five_hour_reset);
        self.seven_day_reset = reset("7d").or(self.seven_day_reset);
    }
}

fn parse_timestamp(text: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    let zone = &text[text.get(19..)?.find(['Z', '+', '-'])? + 19..];
    let offset = match zone.as_bytes().first()? {
        b'Z' => 0,
        sign => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            let offset = hours * 3_600 + minutes * 60;
            if *sign == b'-' { -offset } else { offset }
        }
    };
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second - offset).ok()
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

fn estimate_prompt_tokens(system: &[Value], tools: &Value) -> u64 {
    let characters: usize = system
        .iter()
        .map(|block| block["text"].as_str().unwrap_or_default().chars().count())
        .sum::<usize>()
        + tools.to_string().chars().count();
    (characters as u64).div_ceil(4)
}

fn estimate_text_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn scale_parts(parts: Vec<(&'static str, u64)>, total: u64) -> Vec<(&'static str, u64)> {
    let estimated: u64 = parts.iter().map(|(_, tokens)| tokens).sum();
    if estimated == 0 {
        return parts;
    }
    parts
        .into_iter()
        .map(|(name, tokens)| {
            let scaled = (tokens as f64 * total as f64 / estimated as f64).round();
            (name, scaled as u64)
        })
        .collect()
}

pub struct ContextUse {
    pub parts: Vec<(&'static str, u64)>,
    pub total: u64,
    pub window: u64,
}

fn context_tokens(messages: &[Value], system_tokens: u64) -> u64 {
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
    let (start, usage_tokens) =
        last_usage.map_or((0, system_tokens), |(index, tokens)| (index + 1, tokens));
    usage_tokens + messages[start..].iter().map(estimate_tokens).sum::<u64>()
}

fn cache_hit_rate(messages: &[Value]) -> Option<f64> {
    messages
        .iter()
        .rfind(|message| message["role"] == "assistant" && message.get("stop_reason").is_none())
        .map(|message| Usage::from_json(&message["usage"]))
        .and_then(|usage| {
            let prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
            (prompt_tokens > 0).then(|| usage.cache_read as f64 / prompt_tokens as f64 * 100.0)
        })
}

fn tokens_per_second(messages: &[Value]) -> Option<f64> {
    let mut timed = Usage::default();
    for message in messages
        .iter()
        .filter(|message| message["role"] == "assistant")
    {
        let usage = Usage::from_json(&message["usage"]);
        if usage.duration_ms > 0 {
            timed.add(usage);
        }
    }
    (timed.duration_ms > 0).then(|| timed.output as f64 * 1_000.0 / timed.duration_ms as f64)
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

async fn fetch_quota(http: reqwest::Client, token: String) -> Result<Quota> {
    let response = http
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        bail!("{status}: {}", response.text().await?);
    }
    let body: Value = response.json().await?;
    let remaining = |window: &str| {
        body[window]["utilization"]
            .as_f64()
            .map(|utilisation| 100.0 - utilisation)
    };
    let reset = |window: &str| body[window]["resets_at"].as_str().and_then(parse_timestamp);
    Ok(Quota {
        five_hour_remaining: remaining("five_hour"),
        seven_day_remaining: remaining("seven_day"),
        five_hour_reset: reset("five_hour"),
        seven_day_reset: reset("seven_day"),
    })
}

fn cancel_point(messages: &[Value], checkpoint: usize) -> usize {
    messages[checkpoint..]
        .iter()
        .rposition(|message| message["role"] == "user" || is_compaction(message))
        .map_or(checkpoint, |index| checkpoint + index + 1)
}

pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ToolStart {
        name: String,
        summary: String,
        diff: Option<String>,
    },
    ToolDone {
        name: String,
        error: Option<String>,
        note: Option<String>,
    },
    Stats(Stats),
    Notice(String),
    Queued(String),
    Question {
        questions: Vec<Question>,
        reply: tokio::sync::oneshot::Sender<Vec<Vec<String>>>,
    },
}

pub fn tool_definitions(mcp: &Mcp, ask_user: bool) -> Value {
    let mut definitions = tools::definitions();
    if let Value::Array(list) = &mut definitions {
        if ask_user {
            list.push(ask::definition());
        }
        list.extend(mcp.definitions());
    }
    definitions
}

pub async fn ask_user(
    input: &Value,
    on_event: &mut impl FnMut(AgentEvent),
) -> Result<String, String> {
    let questions = ask::parse(input)?;
    let (reply, answers) = tokio::sync::oneshot::channel();
    on_event(AgentEvent::Question {
        questions: questions.clone(),
        reply,
    });
    Ok(match answers.await {
        Ok(answers) => ask::format_answers(&questions, &answers),
        Err(_) => ask::DECLINED.into(),
    })
}

pub fn shell_message(command: &str, output: &str) -> String {
    format!("<bash-input>{command}</bash-input>\n<bash-output>{output}</bash-output>")
}

pub fn parse_shell_message(text: &str) -> Option<(&str, &str)> {
    let (command, rest) = text
        .strip_prefix("<bash-input>")?
        .split_once("</bash-input>\n<bash-output>")?;
    Some((command, rest.strip_suffix("</bash-output>")?))
}

pub struct Queued {
    pub prompt: String,
    pub images: Vec<(usize, Image)>,
}

pub type Queue = Arc<Mutex<Vec<Queued>>>;

pub fn take_queued(queue: &Queue) -> Vec<Queued> {
    queue
        .lock()
        .map(|mut queued| std::mem::take(&mut *queued))
        .unwrap_or_default()
}

pub struct Agent {
    http: reqwest::Client,
    credentials: Credentials,
    pub model: String,
    pub thinking_level: &'static str,
    messages: Vec<Value>,
    pending_usage: Usage,
    quota: Quota,
    pub instructions: Vec<Instructions>,
    pub skills: Skills,
    pub mcp: Mcp,
    pub ask_user: bool,
    pub session: Session,
    pub queue: Queue,
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
            quota: Quota::default(),
            instructions: load_instructions(),
            skills: skills::load(),
            mcp: Mcp::default(),
            ask_user: false,
            session: Session::new()?,
            queue: Queue::default(),
        })
    }

    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    pub fn history_len(&self) -> usize {
        self.messages.len()
    }

    pub fn take_renewed(&mut self) -> bool {
        self.credentials.take_renewed()
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

    pub async fn quota_request(
        &mut self,
    ) -> Result<impl Future<Output = Result<Quota>> + Send + 'static> {
        let token = self.credentials.access_token(&self.http).await?;
        Ok(fetch_quota(self.http.clone(), token))
    }

    pub fn merge_quota(&mut self, quota: Quota) {
        self.quota = Quota {
            five_hour_remaining: self.quota.five_hour_remaining.or(quota.five_hour_remaining),
            seven_day_remaining: self.quota.seven_day_remaining.or(quota.seven_day_remaining),
            five_hour_reset: self.quota.five_hour_reset.or(quota.five_hour_reset),
            seven_day_reset: self.quota.seven_day_reset.or(quota.seven_day_reset),
        };
    }

    pub fn new_session(&mut self) -> Result<()> {
        self.session = Session::new()?;
        self.messages.clear();
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        Stats {
            usage: total_usage(&self.messages),
            cache_hit_rate: cache_hit_rate(&self.messages),
            tokens_per_second: tokens_per_second(&self.messages),
            context_tokens: context_tokens(
                &active_messages(&self.messages),
                estimate_prompt_tokens(
                    &self.system_prompt(),
                    &tool_definitions(&self.mcp, self.ask_user),
                ),
            ),
            context_window: models::context_window(&self.model),
            quota: self.quota,
        }
    }

    pub fn context_use(&self) -> ContextUse {
        let stats = self.stats();
        let instructions = self
            .instructions
            .iter()
            .map(|instructions| estimate_text_tokens(&instructions.text))
            .sum();
        let skills = skills::format_for_prompt(&self.skills.skills)
            .map_or(0, |text| estimate_text_tokens(&text));
        let mcp_tools = self
            .mcp
            .definitions()
            .map(|definition| estimate_text_tokens(&definition.to_string()))
            .sum();
        let messages = active_messages(&self.messages)
            .iter()
            .map(estimate_tokens)
            .sum();
        let parts = vec![
            (
                "system prompt",
                estimate_text_tokens(IDENTITY) + estimate_text_tokens(SYSTEM_PROMPT),
            ),
            ("instructions", instructions),
            ("skills", skills),
            (
                "built-in tools",
                estimate_text_tokens(&tools::definitions().to_string()),
            ),
            ("MCP tools", mcp_tools),
            ("messages", messages),
        ];
        ContextUse {
            parts: scale_parts(parts, stats.context_tokens),
            total: stats.context_tokens,
            window: stats.context_window,
        }
    }

    fn system_prompt(&self) -> Vec<Value> {
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
        if let Some(text) = skills::format_for_prompt(&self.skills.skills) {
            system.push(json!({ "type": "text", "text": text }));
        }
        system
    }

    fn discard_from(&mut self, index: usize, stop_reason: &str, always_mark: bool) {
        let index = index.min(self.messages.len());
        let mut lost = total_usage(&self.messages[index..]);
        lost.add(std::mem::take(&mut self.pending_usage));
        self.messages.truncate(index);
        if always_mark || lost.input + lost.output + lost.cache_read + lost.cache_write > 0 {
            self.messages.push(json!({
                "role": "assistant",
                "stop_reason": stop_reason,
                "content": [],
                "usage": lost.to_json(),
            }));
        }
    }

    pub fn cancel(&mut self, checkpoint: usize) -> Result<()> {
        self.discard_from(cancel_point(&self.messages, checkpoint), "aborted", true);
        self.session.save(&self.messages)
    }

    pub async fn prompt(
        &mut self,
        prompt: &str,
        images: &[Image],
        on_event: impl FnMut(AgentEvent),
    ) -> Result<()> {
        let prompt = skills::expand_command(prompt, &self.skills.skills)?;
        let checkpoint = self.messages.len();
        let result = self.run(&prompt, images, on_event).await;
        if result.is_err() {
            self.discard_from(cancel_point(&self.messages, checkpoint), "error", false);
            self.session.save(&self.messages)?;
            return result;
        }
        self.session.save(&self.messages)
    }

    pub async fn shell(&mut self, command: &str) -> Result<String> {
        let output = match tools::call("bash", &json!({ "command": command })).await {
            Ok(output) | Err(output) => output,
        };
        self.messages.push(json!({
            "role": "user",
            "content": shell_message(command, &output),
        }));
        self.session.save(&self.messages)?;
        Ok(output)
    }

    pub async fn compact(&mut self, mut on_event: impl FnMut(AgentEvent)) -> Result<()> {
        let end = self.messages.len();
        let result = self.compact_history(end, &mut on_event).await;
        if result.is_err() {
            self.discard_from(self.messages.len(), "error", false);
        }
        self.session.save(&self.messages)?;
        result
    }

    fn context_full(&self) -> bool {
        let stats = self.stats();
        stats.context_window > 0
            && stats.context_tokens * 100 >= stats.context_window * COMPACT_AT_PERCENT
    }

    async fn compact_if_full(
        &mut self,
        end: usize,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<()> {
        if self.context_full() && has_uncompacted(&self.messages[..end]) {
            self.compact_history(end, on_event).await?;
        }
        Ok(())
    }

    async fn compact_history(
        &mut self,
        end: usize,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<()> {
        if !has_uncompacted(&self.messages[..end]) {
            bail!("nothing to compact");
        }
        on_event(AgentEvent::Notice("compacting conversation".into()));
        let mut messages = with_cache_breakpoint(&active_messages(&self.messages[..end]));
        messages.push(json!({ "role": "user", "content": COMPACT_PROMPT }));
        let (content, _, usage) = self
            .request(messages, Some(json!({ "type": "none" })), &mut |event| {
                if let AgentEvent::Notice(_) = event {
                    on_event(event);
                }
            })
            .await?;
        let summary: String = content
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect();
        if summary.trim().is_empty() {
            bail!("compaction returned no summary");
        }
        self.messages.insert(
            end,
            json!({
                "role": "assistant",
                "stop_reason": "compacted",
                "content": [],
                "summary": summary,
                "usage": usage.to_json(),
            }),
        );
        self.pending_usage = Usage::default();
        on_event(AgentEvent::Notice("compacted conversation".into()));
        on_event(AgentEvent::Stats(self.stats()));
        Ok(())
    }

    async fn request(
        &mut self,
        messages: Vec<Value>,
        tool_choice: Option<Value>,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<(Vec<Value>, String, Usage)> {
        let mut messages = messages;
        self.session.inline_images(&mut messages)?;
        let mut attempt = 0;
        loop {
            match self
                .stream_message(&messages, tool_choice.as_ref(), on_event)
                .await
            {
                Ok(reply) => return Ok(reply),
                Err(error) => {
                    let Some(delay) = retry_delay(&error, attempt) else {
                        return Err(error);
                    };
                    attempt += 1;
                    on_event(AgentEvent::Notice(format!(
                        "{error}; retrying in {}s ({attempt}/{MAX_RETRIES})",
                        delay.as_secs()
                    )));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn run(
        &mut self,
        prompt: &str,
        images: &[Image],
        mut on_event: impl FnMut(AgentEvent),
    ) -> Result<()> {
        let content = if images.is_empty() {
            json!(prompt)
        } else {
            let mut blocks = images
                .iter()
                .map(|image| self.session.save_image(image))
                .collect::<Result<Vec<_>>>()?;
            blocks.push(json!({ "type": "text", "text": prompt }));
            Value::Array(blocks)
        };
        self.messages
            .push(json!({ "role": "user", "content": content }));
        on_event(AgentEvent::Stats(self.stats()));
        self.compact_if_full(self.messages.len() - 1, &mut on_event)
            .await?;

        loop {
            let messages = with_cache_breakpoint(&active_messages(&self.messages));
            let (content, stop_reason, usage) = self.request(messages, None, &mut on_event).await?;
            self.messages
                .push(json!({ "role": "assistant", "content": content, "usage": usage.to_json() }));
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
                    diff: tools::diff(name, &block["input"]),
                });
                let result = if name == ask::NAME && self.ask_user {
                    ask_user(&block["input"], &mut on_event).await
                } else {
                    match self.mcp.call(name, &block["input"]).await {
                        Some(result) => result,
                        None => tools::call(name, &block["input"]).await,
                    }
                };
                let (text, is_error) = match result {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                on_event(AgentEvent::ToolDone {
                    name: name.into(),
                    error: is_error.then(|| text.clone()),
                    note: tools::note(name, &text).filter(|_| !is_error),
                });
                results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": block["id"],
                    "content": text,
                    "is_error": is_error,
                }));
            }
            for queued in take_queued(&self.queue) {
                for (_, image) in &queued.images {
                    results.push(self.session.save_image(image)?);
                }
                results.push(json!({ "type": "text", "text": queued.prompt }));
                on_event(AgentEvent::Queued(queued.prompt));
            }
            self.messages
                .push(json!({ "role": "user", "content": results }));
            self.compact_if_full(self.messages.len(), &mut on_event)
                .await?;
        }
    }

    async fn stream_message(
        &mut self,
        messages: &[Value],
        tool_choice: Option<&Value>,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<(Vec<Value>, String, Usage)> {
        let token = self.credentials.access_token(&self.http).await?;
        if self.credentials.take_renewed() {
            on_event(AgentEvent::Notice("renewed sign-in token".into()));
        }
        let mut system = self.system_prompt();
        if let Some(last) = system.last_mut() {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": models::max_output(&self.model),
            "stream": true,
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": self.thinking_level },
            "system": system,
            "tools": tool_definitions(&self.mcp, self.ask_user),
            "messages": messages,
        });
        if let Some(tool_choice) = tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        let response = self
            .http
            .post(API_URL)
            .bearer_auth(token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20")
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
        self.quota.update_from_headers(response.headers());
        if !response.status().is_success() {
            let status = response.status();
            let delay = retry_after(response.headers());
            let message = format!("{status}: {}", response.text().await?);
            if is_retryable_status(status) {
                return Err(retryable(message, delay));
            }
            bail!(message);
        }

        let mut content: Vec<Value> = Vec::new();
        let mut partial_json = String::new();
        let mut input_error = None;
        let mut stop_reason = String::new();
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
                        self.pending_usage = usage;
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
                        self.pending_usage = usage;
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
        Ok((content, stop_reason, usage))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AgentEvent, SYSTEM_PROMPT, ask_user, tool_definitions};
    use super::{
        Quota, active_messages, cache_hit_rate, cancel_point, context_tokens, has_uncompacted,
        parse_shell_message, parse_timestamp, retry_after, retry_delay, retryable, scale_parts,
        shell_message, tokens_per_second, with_cache_breakpoint,
    };
    use crate::{ask, mcp::Mcp};
    use std::time::Duration;

    fn tool_names(definitions: serde_json::Value) -> Vec<String> {
        definitions
            .as_array()
            .unwrap()
            .iter()
            .map(|definition| definition["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn tells_the_model_to_prefer_ast_grep_for_code_search() {
        assert!(SYSTEM_PROMPT.contains(
            "Search code with ast-grep. Fall back to ripgrep for plain text, comments, strings and files ast-grep cannot parse."
        ));
    }

    #[test]
    fn offers_the_question_tool_only_when_someone_can_answer() {
        let mcp = Mcp::default();
        assert!(!tool_names(tool_definitions(&mcp, false)).contains(&ask::NAME.to_string()));
        assert!(tool_names(tool_definitions(&mcp, true)).contains(&ask::NAME.to_string()));
    }

    fn question_input() -> serde_json::Value {
        json!({ "questions": [{
            "question": "Which output?",
            "header": "Output",
            "options": [{ "label": "JSON", "description": "" }, { "label": "Text", "description": "" }]
        }] })
    }

    #[tokio::test]
    async fn returns_the_answers_from_the_user() {
        let mut asked = Vec::new();
        let result = ask_user(&question_input(), &mut |event| {
            if let AgentEvent::Question { questions, reply } = event {
                asked = questions;
                let _ = reply.send(vec![vec!["Text".to_string()]]);
            }
        })
        .await;
        assert_eq!(result, Ok("Which output? → Text".to_string()));
        assert_eq!(asked[0].header, "Output");
    }

    #[tokio::test]
    async fn reports_when_the_user_declines() {
        let result = ask_user(&question_input(), &mut |_| {}).await;
        assert_eq!(result, Ok(ask::DECLINED.to_string()));
    }

    #[tokio::test]
    async fn rejects_invalid_questions() {
        let result = ask_user(&json!({ "questions": [] }), &mut |_| {}).await;
        assert_eq!(result, Err("ask between 1 and 4 questions".to_string()));
    }

    #[test]
    fn scales_context_parts_to_total() {
        assert_eq!(
            scale_parts(vec![("a", 10), ("b", 30)], 80),
            vec![("a", 20), ("b", 60)]
        );
        assert_eq!(scale_parts(vec![("a", 0)], 80), vec![("a", 0)]);
    }

    #[test]
    fn parses_shell_messages() {
        let text = shell_message("ls -a", "a\nb\n");
        assert_eq!(parse_shell_message(&text), Some(("ls -a", "a\nb\n")));
        assert_eq!(parse_shell_message("hello"), None);
    }

    #[test]
    fn backs_off_on_retryable_errors() {
        let error = retryable("overloaded", None);
        let delays: Vec<_> = (0..4).map(|attempt| retry_delay(&error, attempt)).collect();
        assert_eq!(
            delays,
            [
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(4)),
                None
            ]
        );
        assert_eq!(retry_delay(&anyhow::anyhow!("bad request"), 0), None);
    }

    #[test]
    fn follows_short_retry_after() {
        let short = retryable("rate limited", Some(Duration::from_secs(10)));
        assert_eq!(retry_delay(&short, 0), Some(Duration::from_secs(10)));
        let long = retryable("rate limited", Some(Duration::from_secs(3600)));
        assert_eq!(retry_delay(&long, 0), None);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(7)));
    }

    #[test]
    fn keeps_prompt_and_finished_tool_rounds_on_cancel() {
        let prompt = json!({ "role": "user", "content": "hello" });
        let tool_use = json!({ "role": "assistant", "content": [{ "type": "tool_use" }] });
        let tool_result = json!({ "role": "user", "content": [{ "type": "tool_result" }] });
        assert_eq!(cancel_point(std::slice::from_ref(&prompt), 0), 1);
        let compaction = json!({ "role": "assistant", "stop_reason": "compacted", "content": [] });
        assert_eq!(
            cancel_point(&[compaction.clone(), prompt.clone(), tool_use.clone()], 0),
            2
        );
        assert_eq!(
            cancel_point(&[prompt.clone(), compaction, tool_use.clone()], 0),
            2
        );
        assert_eq!(
            cancel_point(
                &[prompt.clone(), tool_use.clone(), tool_result, tool_use],
                0
            ),
            3
        );
    }

    #[test]
    fn replaces_compacted_messages_with_summary() {
        let prompt = json!({ "role": "user", "content": "hello" });
        let reply = json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] });
        let compaction = json!({
            "role": "assistant",
            "stop_reason": "compacted",
            "content": [],
            "summary": "greeted",
        });
        let messages = vec![prompt.clone(), reply, compaction.clone(), prompt.clone()];
        let active = active_messages(&messages);
        assert_eq!(active.len(), 2);
        assert!(active[0]["content"].as_str().unwrap().ends_with("greeted"));
        assert_eq!(active[1], prompt);
        assert!(has_uncompacted(&messages));
        assert!(!has_uncompacted(&messages[..3]));
        assert!(!has_uncompacted(&[]));
    }

    #[test]
    fn drops_empty_assistant_replies_from_requests() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [], "usage": { "output": 3 } }),
            json!({ "role": "user", "content": "again" }),
        ];
        let request = with_cache_breakpoint(&messages);
        assert_eq!(request.len(), 2);
        assert_eq!(request[1]["content"][0]["text"], "again");
    }

    #[test]
    fn keeps_quota_when_headers_are_missing() {
        let mut quota = Quota {
            five_hour_remaining: Some(40.0),
            seven_day_remaining: Some(70.0),
            five_hour_reset: Some(100),
            seven_day_reset: Some(200),
        };
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            "0.25".parse().unwrap(),
        );
        quota.update_from_headers(&headers);
        assert_eq!(quota.five_hour_remaining, Some(75.0));
        assert_eq!(quota.seven_day_remaining, Some(70.0));
        assert_eq!(quota.five_hour_reset, Some(100));
        assert_eq!(quota.seven_day_reset, Some(200));
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(
            parse_timestamp("2026-10-09T04:39:59.870710+00:00"),
            Some(1_791_520_799)
        );
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(
            parse_timestamp("2024-02-29T00:00:00-00:30"),
            Some(1_709_166_600)
        );
    }

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
        assert_eq!(context_tokens(&messages, 50), 137);
    }

    #[test]
    fn ignores_discarded_usage_in_cache_hit_rate() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hi" }],
                "usage": { "input": 10, "output": 5, "cache_read": 90, "cache_write": 0 },
            }),
            json!({
                "role": "assistant",
                "stop_reason": "aborted",
                "content": [],
                "usage": { "input": 999 },
            }),
        ];
        assert_eq!(cache_hit_rate(&messages), Some(90.0));
    }

    #[test]
    fn estimates_context_without_usage() {
        let messages = vec![json!({ "role": "user", "content": "12345" })];
        assert_eq!(context_tokens(&messages, 50), 52);
    }

    #[test]
    fn averages_tokens_per_second_over_timed_replies() {
        let messages = vec![
            json!({ "role": "assistant", "content": [], "usage": { "output": 500 } }),
            json!({ "role": "assistant", "content": [], "usage": { "output": 100, "duration_ms": 1_000 } }),
            json!({ "role": "assistant", "content": [], "usage": { "output": 200, "duration_ms": 3_000 } }),
        ];
        assert_eq!(tokens_per_second(&messages), Some(75.0));
        assert_eq!(tokens_per_second(&messages[..1]), None);
    }
}
