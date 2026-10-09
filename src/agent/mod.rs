use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use serde_json::{Value, json};

mod context;
mod retry;
mod stream;
mod system;
mod usage;

use crate::{
    auth::Credentials,
    images::Image,
    mcp::Mcp,
    models,
    session::{Session, SessionSummary},
    skills::{self, Skills},
    tools,
};
pub use context::ContextUse;
use context::{
    active_messages, context_tokens, estimate_prompt_tokens, estimate_text_tokens, estimate_tokens,
    has_uncompacted, is_compaction, scale_parts, with_cache_breakpoint,
};
use retry::{MAX_RETRIES, retry_delay};
pub use system::Instructions;
use system::{IDENTITY, load_instructions, missing_search_programs, system_text};
pub use usage::{Quota, Stats, Usage};
use usage::{cache_hit_rate, fetch_quota, tokens_per_second, total_usage};

const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1000";

pub const THINKING_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const DEFAULT_THINKING_LEVEL: &str = "medium";
const COMPACT_PROMPT: &str = "Summarise this conversation so that you can continue the work from the summary alone. Include the user's requests, decisions made, files read and changed, the current state of the work and the next steps. Do not call tools. Reply with the summary only.";
const COMPACT_AT_PERCENT: u64 = 80;

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
}

pub fn tool_definitions(mcp: &Mcp) -> Value {
    let mut definitions = tools::definitions();
    if let Value::Array(list) = &mut definitions {
        list.extend(mcp.definitions());
    }
    definitions
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
    pub session: Session,
    pub queue: Queue,
    pub missing_programs: Vec<&'static str>,
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
            session: Session::new()?,
            queue: Queue::default(),
            missing_programs: missing_search_programs(std::env::var_os("PATH")),
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
                estimate_prompt_tokens(&self.system_prompt(), &tool_definitions(&self.mcp)),
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
                estimate_text_tokens(IDENTITY)
                    + estimate_text_tokens(&system_text(&self.missing_programs)),
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
            json!({ "type": "text", "text": system_text(&self.missing_programs) }),
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
                let result = match self.mcp.call(name, &block["input"]).await {
                    Some(result) => result,
                    None => tools::call(name, &block["input"]).await,
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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::tool_definitions;
    use super::{cancel_point, parse_shell_message, shell_message};
    use crate::mcp::Mcp;

    fn tool_names(definitions: serde_json::Value) -> Vec<String> {
        definitions
            .as_array()
            .unwrap()
            .iter()
            .map(|definition| definition["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn does_not_offer_a_question_tool() {
        let names = tool_names(tool_definitions(&Mcp::default()));
        assert!(names.contains(&"bash".to_string()));
        assert!(!names.contains(&"ask_user_question".to_string()));
    }

    #[test]
    fn parses_shell_messages() {
        let text = shell_message("ls -a", "a\nb\n");
        assert_eq!(parse_shell_message(&text), Some(("ls -a", "a\nb\n")));
        assert_eq!(parse_shell_message("hello"), None);
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
}
