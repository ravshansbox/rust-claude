use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

mod background;
mod compact;
mod context;
pub(crate) mod retry;
mod stream;
mod subagent;
pub(crate) use subagent::is_failure;
mod system;
#[cfg(test)]
pub(crate) mod test_support;
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
use context::{
    active_messages, context_tokens, has_uncompacted, is_compaction, with_cache_breakpoint,
};
use retry::{MAX_RETRIES, is_prompt_too_long, is_thinking_mismatch, retry_delay};
use stream::Response;
pub use system::Instructions;
use system::{IDENTITY, SYSTEM_PROMPT, load_instructions};
pub use usage::{Quota, Stats, Usage};
use usage::{fetch_quota, tokens_per_second, total_usage};

const API_BASE: &str = "https://api.anthropic.com";

pub const DEFAULT_EFFORT: &str = "medium";

/// The result saved for a tool call that was cancelled before it finished.
pub const CANCELLED: &str = "cancelled by the user";

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
    /// A subagent started on task `task`, counting from 0.
    TaskStart {
        task: usize,
        description: String,
    },
    /// The subagent on `task` has made `count` tool calls so far.
    TaskToolCalls {
        task: usize,
        count: usize,
    },
    /// The latest stats of the subagent on `task`.
    TaskStats {
        task: usize,
        stats: Stats,
    },
    /// The subagent on `task` finished with `reply` after `tool_calls` tool
    /// calls, or failed or did not run, as the error tells the main agent.
    TaskDone {
        task: usize,
        reply: Result<String, String>,
        tool_calls: usize,
    },
}

fn text_of(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect()
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

/// Gives up on a connection attempt after this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Gives up when no data arrives for this long, so a dead connection fails
/// and gets retried instead of hanging. Long, because the API can take a
/// while to start replying to a large prompt.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

pub fn http_client() -> Result<reqwest::Client> {
    http_client_with(READ_TIMEOUT)
}

fn http_client_with(read_timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(read_timeout)
        .build()?)
}

pub struct Agent {
    http: reqwest::Client,
    /// Where API requests go. Tests point this at a local server.
    pub(crate) api_base: String,
    credentials: Credentials,
    pub model: String,
    pub effort: &'static str,
    messages: Vec<Value>,
    pending_usage: Usage,
    /// Results of the tool calls in the current round that have finished.
    tool_results: Vec<Value>,
    quota: Quota,
    pub instructions: Vec<Instructions>,
    pub skills: Skills,
    pub mcp: Mcp,
    pub session: Session,
    pub queue: Queue,
    /// Tokens used by each subagent started since the last tool results.
    subagent_usage: Arc<Mutex<Vec<Usage>>>,
    /// Whether this agent runs a task for another agent. A subagent cannot
    /// start subagents or background commands, and its conversation is not saved.
    subagent: bool,
    background: tools::background::Background,
}

impl Agent {
    pub fn new(http: reqwest::Client, credentials: Credentials, model: String) -> Result<Self> {
        Ok(Self {
            http,
            api_base: API_BASE.into(),
            credentials,
            model,
            effort: DEFAULT_EFFORT,
            messages: Vec::new(),
            pending_usage: Usage::default(),
            tool_results: Vec::new(),
            quota: Quota::default(),
            instructions: load_instructions(),
            skills: skills::load(),
            mcp: Mcp::default(),
            session: Session::new()?,
            queue: Queue::default(),
            subagent_usage: Arc::default(),
            subagent: false,
            background: tools::background::Background::default(),
        })
    }

    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    pub fn history_len(&self) -> usize {
        self.messages.len()
    }

    pub fn take_renewal_notice(&mut self) -> Option<String> {
        self.credentials.take_renewal_notice()
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let id = self.session.id.clone();
        tokio::task::spawn_blocking(move || Session::list_except(&id)).await?
    }

    pub async fn resume(&mut self, id: &str) -> Result<Vec<Value>> {
        let id = id.to_string();
        let (session, messages) = tokio::task::spawn_blocking(move || Session::load(&id)).await??;
        self.session = session;
        self.messages.clone_from(&messages);
        self.background = tools::background::Background::default();
        Ok(messages)
    }

    pub async fn quota_request(
        &mut self,
    ) -> Result<impl Future<Output = Result<Quota>> + Send + 'static> {
        let token = self.credentials.access_token(&self.http).await?;
        Ok(fetch_quota(
            self.http.clone(),
            format!("{}/api/oauth/usage", self.api_base),
            token,
        ))
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
        self.background = tools::background::Background::default();
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        let mut usage = total_usage(&self.messages);
        usage.add(self.running_subagent_usage());
        Stats {
            usage,
            tokens_per_second: tokens_per_second(&self.messages),
            context_tokens: context_tokens(&active_messages(&self.messages)),
            context_window: models::context_window(&self.model),
            quota: self.quota,
            background: self.background.len(),
        }
    }

    fn running_subagent_usage(&self) -> Usage {
        let mut total = Usage::default();
        if let Ok(slots) = self.subagent_usage.lock() {
            slots.iter().for_each(|usage| total.add(*usage));
        }
        total
    }

    fn tool_definitions(&self) -> Value {
        let mut definitions = tools::definitions();
        if let Value::Array(list) = &mut definitions {
            if self.subagent {
                list.retain(|definition| {
                    definition["name"] != tools::agent::NAME
                        && definition["name"] != tools::background::STOP
                });
                for definition in list.iter_mut() {
                    if let Some(properties) = definition
                        .pointer_mut("/input_schema/properties")
                        .and_then(Value::as_object_mut)
                    {
                        properties.remove(tools::background::RUN_IN_BACKGROUND);
                    }
                }
            }
            list.extend(self.mcp.definitions());
        }
        definitions
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
        if always_mark || lost.tokens() > 0 {
            self.messages.push(json!({
                "role": "assistant",
                "stop_reason": stop_reason,
                "content": [],
                "usage": lost,
            }));
        }
    }

    /// Answers each tool call of the last reply: with its result when the
    /// call finished, or as cancelled when it did not.
    fn push_tool_results(&mut self) {
        let mut results = std::mem::take(&mut self.tool_results);
        let Some(reply) = self.messages.last() else {
            return;
        };
        if reply["role"] != "assistant" || reply.get("stop_reason").is_some() {
            return;
        }
        let unfinished = reply["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "tool_use")
            .skip(results.len());
        for block in unfinished {
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": block["id"],
                "content": CANCELLED,
                "is_error": true,
            }));
        }
        if !results.is_empty() {
            let mut message = json!({ "role": "user", "content": results });
            let usage = self.running_subagent_usage();
            if usage.tokens() > 0 {
                message["usage"] = json!(usage);
            }
            self.messages.push(message);
        }
        if let Ok(mut slots) = self.subagent_usage.lock() {
            slots.clear();
        }
    }

    pub fn cancel(&mut self, checkpoint: usize) -> Result<()> {
        self.push_tool_results();
        self.discard_from(cancel_point(&self.messages, checkpoint), "aborted", true);
        self.session.save(&self.messages)
    }

    /// Saves a prompt that was cancelled before it could be sent, marked as
    /// cancelled like one stopped while running.
    pub fn cancel_unsent(&mut self, prompt: &str, images: &[Image]) -> Result<()> {
        let prompt = skills::expand_command(prompt, &self.skills.skills)?;
        let checkpoint = self.messages.len();
        self.push_prompt(&prompt, images)?;
        self.cancel(checkpoint)
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
        if let Err(error) = result {
            self.discard_from(cancel_point(&self.messages, checkpoint), "error", false);
            return Err(self.save_after_failure(error));
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

    /// Saves the session after `error` stopped a prompt or compaction. A save
    /// that also fails is reported after `error`, which stays visible.
    fn save_after_failure(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.session.save(&self.messages) {
            Ok(()) => error,
            Err(save_error) => anyhow!("{error:#}; failed to save session: {save_error:#}"),
        }
    }

    async fn request(
        &mut self,
        messages: Vec<Value>,
        tool_choice: Option<Value>,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Response> {
        let mut messages = messages;
        self.session.inline_images(&mut messages)?;
        let mut attempt = 0;
        loop {
            match self
                .stream_message(&messages, tool_choice.as_ref(), on_event)
                .await
            {
                Ok(reply) => return Ok(reply),
                Err(error)
                    if is_thinking_mismatch(&error)
                        && !self.session.drops_mismatched_thinking() =>
                {
                    self.session.drop_mismatched_thinking();
                    on_event(AgentEvent::Notice(
                        "earlier thinking no longer matches the conversation; retrying without it"
                            .into(),
                    ));
                }
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

    fn push_prompt(&mut self, prompt: &str, images: &[Image]) -> Result<()> {
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
        Ok(())
    }

    async fn run(
        &mut self,
        prompt: &str,
        images: &[Image],
        mut on_event: impl FnMut(AgentEvent),
    ) -> Result<()> {
        self.push_prompt(prompt, images)?;
        on_event(AgentEvent::Stats(self.stats()));
        let mut end = self
            .compact_if_full(self.messages.len() - 1, &mut on_event)
            .await;

        loop {
            let messages = with_cache_breakpoint(&active_messages(&self.messages));
            let response = match self.request(messages, None, &mut on_event).await {
                Err(error)
                    if is_prompt_too_long(&error) && has_uncompacted(&self.messages[..end]) =>
                {
                    self.compact_history(end, &mut on_event)
                        .await
                        .map_err(|compact_error| anyhow!("{error:#}; {compact_error:#}"))?;
                    let messages = with_cache_breakpoint(&active_messages(&self.messages));
                    self.request(messages, None, &mut on_event).await
                }
                response => response,
            };
            let Response {
                content,
                stop_reason,
                usage,
                retried,
            } = response?;
            // A refusal can arrive partway through a reply, so the text so far
            // is incomplete. Failing throws it away and keeps its tokens.
            if stop_reason == "refusal" {
                bail!("the model declined to answer");
            }
            self.messages.push(json!({
                "role": "assistant",
                "content": content,
                "usage": usage.to_json_with_retried(retried),
            }));
            self.pending_usage = Usage::default();
            on_event(AgentEvent::Stats(self.stats()));
            let cut_off = match stop_reason.as_str() {
                "max_tokens" => Some("max_tokens reached"),
                "model_context_window_exceeded" => Some("the context window is full"),
                _ => None,
            };
            if let Some(cause) = cut_off {
                on_event(AgentEvent::Notice(format!("reply cut off: {cause}")));
            }
            if let Some(sources) = tools::web_search::sources(&content) {
                on_event(AgentEvent::Text(sources));
            }
            if stop_reason == "pause_turn" {
                continue;
            }
            if stop_reason != "tool_use" {
                return Ok(());
            }

            for block in content.iter().filter(|block| block["type"] == "tool_use") {
                let name = block["name"].as_str().unwrap_or_default();
                on_event(AgentEvent::ToolStart {
                    name: name.into(),
                    summary: tools::summary(name, &block["input"]),
                    diff: tools::diff(name, &block["input"]),
                });
                let result = if name == tools::agent::NAME && !self.subagent {
                    self.run_tasks(&block["input"], &mut on_event).await
                } else if let Some(result) = self.call_background(name, &block["input"]) {
                    on_event(AgentEvent::Stats(self.stats()));
                    result.map(Value::from)
                } else {
                    match self.mcp.call(name, &block["input"]).await {
                        Some(result) => result,
                        None => tools::call(name, &block["input"]).await,
                    }
                    .map(Value::from)
                };
                let (content, is_error) = match result {
                    Ok(content) => (content, false),
                    Err(text) => (Value::from(text), true),
                };
                let text = content.as_str().unwrap_or_default();
                on_event(AgentEvent::ToolDone {
                    name: name.into(),
                    error: is_error.then(|| text.to_string()),
                    note: tools::note(name, text).filter(|_| !is_error),
                });
                self.tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": block["id"],
                    "content": content,
                    "is_error": is_error,
                }));
            }
            self.push_tool_results();
            if !self.subagent {
                self.session.save(&self.messages)?;
            }
            // Compact before adding queued prompts, so they follow the summary
            // word for word instead of being summarised.
            end = self
                .compact_if_full(self.messages.len(), &mut on_event)
                .await;
            if let Some(prompts) = self.take_queued_prompts(&mut on_event)? {
                self.messages
                    .push(json!({ "role": "user", "content": prompts }));
            }
        }
    }

    /// Turns queued prompts into message blocks. The prompts stay queued when
    /// saving one of their images fails. Images are saved with the queue
    /// unlocked, so the interface does not wait to draw or queue a prompt.
    fn take_queued_prompts(
        &self,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Option<Vec<Value>>> {
        let queued = take_queued(&self.queue);
        if queued.is_empty() {
            return Ok(None);
        }
        let mut blocks = Vec::new();
        let saved = queued.iter().try_for_each(|queued| {
            for (_, image) in &queued.images {
                blocks.push(self.session.save_image(image)?);
            }
            blocks.push(json!({ "type": "text", "text": queued.prompt }));
            Ok::<_, anyhow::Error>(())
        });
        if let Err(error) = saved {
            // Ahead of any prompt queued meanwhile, keeping their order.
            if let Ok(mut queue) = self.queue.lock() {
                queue.splice(0..0, queued);
            }
            return Err(error);
        }
        for queued in queued {
            on_event(AgentEvent::Queued(queued.prompt));
        }
        Ok(Some(blocks))
    }
}

#[cfg(test)]
mod tests;
