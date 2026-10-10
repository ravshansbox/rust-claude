use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

mod context;
pub(crate) mod retry;
mod stream;
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
pub use context::ContextUse;
use context::{
    active_messages, context_tokens, estimate_prompt_tokens, estimate_text_tokens, estimate_tokens,
    has_uncompacted, is_compaction, scale_parts, shorten_tool_results, with_cache_breakpoint,
};
use retry::{MAX_RETRIES, is_prompt_too_long, is_thinking_mismatch, retry_delay};
use stream::Response;
pub use system::Instructions;
use system::{IDENTITY, load_instructions, missing_search_programs, system_text};
pub use usage::{Quota, Stats, Usage};
use usage::{cache_hit_rate, fetch_quota, tokens_per_second, total_usage};

const API_BASE: &str = "https://api.anthropic.com";

pub const THINKING_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const DEFAULT_THINKING_LEVEL: &str = "medium";
const COMPACT_PROMPT: &str = "Summarise this conversation so that you can continue the work from the summary alone. Include the user's requests, decisions made, files read and changed, the current state of the work and the next steps. Do not call tools. Reply with the summary only.";
const COMPACT_AT_PERCENT: u64 = 80;
/// How many characters of each tool output to keep, in turn, when the
/// conversation is too long to summarise.
const COMPACT_TOOL_RESULT_LIMITS: [usize; 2] = [2_000, 200];

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
    pub thinking_level: &'static str,
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
    pub missing_programs: Vec<&'static str>,
}

impl Agent {
    pub fn new(http: reqwest::Client, credentials: Credentials, model: String) -> Result<Self> {
        Ok(Self {
            http,
            api_base: API_BASE.into(),
            credentials,
            model,
            thinking_level: DEFAULT_THINKING_LEVEL,
            messages: Vec::new(),
            pending_usage: Usage::default(),
            tool_results: Vec::new(),
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

    pub fn take_renewal_notice(&mut self) -> Option<String> {
        self.credentials.take_renewal_notice()
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
            .get(format!("{}/v1/models?limit=1000", self.api_base))
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
                "content": "cancelled by the user",
                "is_error": true,
            }));
        }
        if !results.is_empty() {
            self.messages
                .push(json!({ "role": "user", "content": results }));
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

    pub async fn compact(&mut self, mut on_event: impl FnMut(AgentEvent)) -> Result<()> {
        let end = self.messages.len();
        let result = self.compact_history(end, &mut on_event).await;
        if let Err(error) = result {
            self.discard_from(self.messages.len(), "error", false);
            return Err(self.save_after_failure(error));
        }
        self.session.save(&self.messages)
    }

    /// Saves the session after `error` stopped a prompt or compaction. A save
    /// that also fails is reported after `error`, which stays visible.
    fn save_after_failure(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.session.save(&self.messages) {
            Ok(()) => error,
            Err(save_error) => anyhow!("{error:#}; failed to save session: {save_error:#}"),
        }
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
        let active = active_messages(&self.messages[..end]);
        let mut response = self.summarise(&active, on_event).await;
        // A round of tool calls can take the conversation past the context
        // window, and then the API rejects it whole. The summary can do with
        // less of each tool output.
        for limit in COMPACT_TOOL_RESULT_LIMITS {
            if !response.as_ref().is_err_and(is_prompt_too_long) {
                break;
            }
            on_event(AgentEvent::Notice(format!(
                "conversation too long to summarise; retrying with tool output cut to {limit} characters"
            )));
            response = self
                .summarise(&shorten_tool_results(&active, limit), on_event)
                .await;
        }
        let response = response?;
        match response.stop_reason.as_str() {
            "end_turn" => {}
            "refusal" => bail!(
                "the model declined to summarise the conversation; the conversation was not compacted"
            ),
            reason => {
                bail!("the summary was cut off ({reason}); the conversation was not compacted")
            }
        }
        let summary: String = response
            .content
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
                "usage": response.usage.to_json_with_retried(response.retried),
            }),
        );
        self.pending_usage = Usage::default();
        on_event(AgentEvent::Notice("compacted conversation".into()));
        on_event(AgentEvent::Stats(self.stats()));
        Ok(())
    }

    async fn summarise(
        &mut self,
        messages: &[Value],
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Response> {
        let mut messages = with_cache_breakpoint(messages);
        messages.push(json!({ "role": "user", "content": COMPACT_PROMPT }));
        self.request(messages, Some(json!({ "type": "none" })), &mut |event| {
            if let AgentEvent::Notice(_) = event {
                on_event(event);
            }
        })
        .await
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
        self.compact_if_full(self.messages.len() - 1, &mut on_event)
            .await?;

        loop {
            let messages = with_cache_breakpoint(&active_messages(&self.messages));
            let Response {
                content,
                stop_reason,
                usage,
                retried,
            } = self.request(messages, None, &mut on_event).await?;
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
                self.tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": block["id"],
                    "content": text,
                    "is_error": is_error,
                }));
            }
            self.push_tool_results();
            self.session.save(&self.messages)?;
            // Compact before adding queued prompts, so they follow the summary
            // word for word instead of being summarised.
            self.compact_if_full(self.messages.len(), &mut on_event)
                .await?;
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
mod tests {
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

    use super::{
        AgentEvent, Queued, cancel_point, http_client_with, parse_shell_message, shell_message,
        test_support::{self, MockApi, Reply, stopped_reply, text_reply, tool_reply},
    };
    use crate::images::Image;

    #[tokio::test]
    async fn retries_a_reply_that_stops_arriving() {
        let api = MockApi::start(vec![Reply::Stall, text_reply("hello")]).await;
        let http = http_client_with(Duration::from_millis(300)).unwrap();
        let mut agent = test_support::agent(&api, http);
        let mut text = String::new();
        let mut notices = Vec::new();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            agent.prompt("hi", &[], |event| match event {
                AgentEvent::Text(delta) => text.push_str(&delta),
                AgentEvent::Notice(notice) => notices.push(notice),
                _ => {}
            }),
        )
        .await;
        test_support::remove_session(&agent);
        assert!(result.expect("prompt hung").is_ok());
        assert_eq!(text, "hello");
        assert_eq!(api.requests().await.len(), 2);
        assert!(notices.iter().any(|notice| notice.contains("retrying in")));
    }

    #[tokio::test]
    async fn retries_an_overloaded_reply_whose_error_breaks_off() {
        let api = MockApi::start(vec![Reply::CutOffOverloaded, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut text = String::new();
        let result = agent
            .prompt("hi", &[], |event| {
                if let AgentEvent::Text(delta) = event {
                    text.push_str(&delta);
                }
            })
            .await;
        test_support::remove_session(&agent);
        result.unwrap();
        assert_eq!(text, "hello");
        assert_eq!(api.requests().await.len(), 2);
    }

    #[tokio::test]
    async fn says_why_the_api_could_not_be_reached() {
        let api = MockApi::start(vec![]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        // Nothing listens on a port that was just freed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        agent.api_base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let mut notices = Vec::new();
        let result = agent
            .prompt("hi", &[], |event| {
                if let AgentEvent::Notice(notice) = event {
                    notices.push(notice);
                }
            })
            .await;
        test_support::remove_session(&agent);
        let message = result.unwrap_err().to_string();
        assert!(message.to_lowercase().contains("refused"), "{message}");
        assert!(notices[0].to_lowercase().contains("refused"), "{notices:?}");
    }

    #[tokio::test]
    async fn says_when_a_reply_stopped_because_the_context_window_filled_up() {
        let api = MockApi::start(vec![stopped_reply(
            "the start of",
            "model_context_window_exceeded",
        )])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut notices = Vec::new();
        let result = agent
            .prompt("hi", &[], |event| {
                if let AgentEvent::Notice(notice) = event {
                    notices.push(notice);
                }
            })
            .await;
        test_support::remove_session(&agent);
        result.unwrap();
        assert_eq!(
            notices,
            vec!["reply cut off: the context window is full".to_string()]
        );
    }

    #[tokio::test]
    async fn reports_why_a_prompt_failed_when_saving_the_session_also_fails() {
        let api = MockApi::start(vec![Reply::BadRequest("prompt is too long".into())]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        // A session file in a folder that does not exist cannot be written.
        agent.session.id = format!("{}/missing", agent.session.id);
        let error = agent.prompt("hi", &[], |_| {}).await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("prompt is too long"), "{message}");
        assert!(message.contains("failed to save session"), "{message}");
    }

    #[tokio::test]
    async fn counts_tokens_of_a_reply_that_failed_and_was_retried() {
        let overloaded = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 500, "output_tokens": 1 } } }),
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
        ]);
        let api = MockApi::start(vec![overloaded, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let result = agent.prompt("hi", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        assert_eq!(api.requests().await.len(), 2);
        let usage = agent.stats().usage;
        assert_eq!((usage.input, usage.output), (501, 2));
    }

    #[tokio::test]
    async fn keeps_the_conversation_when_the_summary_does_not_finish() {
        for stop_reason in ["max_tokens", "model_context_window_exceeded", "refusal"] {
            let api = MockApi::start(vec![
                text_reply("hello"),
                stopped_reply("partial summary", stop_reason),
                text_reply("ok"),
            ])
            .await;
            let mut agent = test_support::agent(&api, reqwest::Client::new());
            let first = agent.prompt("hi", &[], |_| {}).await;
            let compacted = agent.compact(|_| {}).await;
            let next = agent.prompt("next", &[], |_| {}).await;
            test_support::remove_session(&agent);
            first.unwrap();
            next.unwrap();
            let message = compacted.unwrap_err().to_string();
            assert!(
                message.contains("not compacted"),
                "{stop_reason}: {message}"
            );
            let requests = api.requests().await;
            let after = requests[2]["messages"].to_string();
            assert!(!after.contains("partial summary"), "{stop_reason}: {after}");
            assert!(after.contains("hello"), "{stop_reason}: {after}");
        }
    }

    #[tokio::test]
    async fn discards_a_reply_the_model_declined_partway_through() {
        let refusal = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 10, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial answer" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": { "output_tokens": 5 } }),
            json!({ "type": "message_stop" }),
        ]);
        let api = MockApi::start(vec![refusal, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let declined = agent.prompt("hi", &[], |_| {}).await;
        let next = agent.prompt("again", &[], |_| {}).await;
        test_support::remove_session(&agent);
        let message = declined.unwrap_err().to_string();
        assert!(message.contains("declined to answer"), "{message}");
        next.unwrap();
        let requests = api.requests().await;
        let resent = requests[1]["messages"].to_string();
        assert!(!resent.contains("partial answer"), "{resent}");
        assert!(resent.contains("again"), "{resent}");
        let usage = agent.stats().usage;
        assert_eq!((usage.input, usage.output), (11, 6));
    }

    #[tokio::test]
    async fn says_the_model_declined_when_it_stops_partway_through_a_tool_call() {
        // The tool input may be whole or cut off where the model stopped.
        for partial in [
            "{\"command\": \"touch declined\"}",
            "{\"command\": \"touch declined",
        ] {
            let refusal = Reply::Events(vec![
                json!({ "type": "message_start", "message": { "usage": { "input_tokens": 10, "output_tokens": 1 } } }),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "t1", "name": "bash", "input": {} } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": partial } }),
                json!({ "type": "content_block_stop", "index": 0 }),
                json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": { "output_tokens": 5 } }),
                json!({ "type": "message_stop" }),
            ]);
            let api = MockApi::start(vec![refusal, text_reply("hello")]).await;
            let mut agent = test_support::agent(&api, reqwest::Client::new());
            let declined = agent.prompt("hi", &[], |_| {}).await;
            let next = agent.prompt("again", &[], |_| {}).await;
            test_support::remove_session(&agent);
            let message = declined.unwrap_err().to_string();
            assert!(
                message.contains("declined to answer"),
                "{partial}: {message}"
            );
            next.unwrap();
            let requests = api.requests().await;
            let resent = requests[1]["messages"].to_string();
            assert!(!resent.contains("touch declined"), "{partial}: {resent}");
            let usage = agent.stats().usage;
            assert_eq!((usage.input, usage.output), (11, 6), "{partial}");
        }
    }

    #[tokio::test]
    async fn keeps_tokens_of_a_failed_attempt_out_of_the_context_size() {
        let overloaded = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 500, "cache_read_input_tokens": 500, "output_tokens": 1 } } }),
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
        ]);
        let api = MockApi::start(vec![overloaded, text_reply("hello")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let result = agent.prompt("hi", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        let stats = agent.stats();
        assert_eq!(stats.context_tokens, 2);
        assert_eq!(stats.cache_hit_rate, Some(0.0));
        assert_eq!(stats.usage.cache_read, 500);
    }

    #[tokio::test]
    async fn drops_thinking_bound_to_a_changed_conversation_for_the_rest_of_the_session() {
        let mismatch = "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation. Remove the block, or set `thinking.block_binding.prefix_mismatch_behavior` to \"drop_block\". The `tools` list differs from when the block was created.";
        let api = MockApi::start(vec![
            text_reply("first"),
            Reply::BadRequest(mismatch.into()),
            text_reply("second"),
            text_reply("third"),
            text_reply("after restart"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut notices = Vec::new();
        let first = agent.prompt("one", &[], |_| {}).await;
        let second = agent
            .prompt("two", &[], |event| {
                if let AgentEvent::Notice(notice) = event {
                    notices.push(notice);
                }
            })
            .await;
        let third = agent.prompt("three", &[], |_| {}).await;
        // Close the session, as quitting does, so that it can be resumed.
        let id = agent.session.id.clone();
        drop(agent);
        let mut restarted = test_support::agent(&api, reqwest::Client::new());
        let resumed = restarted.resume(&id);
        let after_restart = restarted.prompt("four", &[], |_| {}).await;
        test_support::remove_session(&restarted);
        first.unwrap();
        second.unwrap();
        third.unwrap();
        resumed.unwrap();
        after_restart.unwrap();
        assert_eq!(notices.len(), 1, "{notices:?}");
        let requests = api.requests().await;
        let headers = api.headers().await;
        assert_eq!(requests.len(), 5);
        let drop_block = json!({ "prefix_mismatch_behavior": "drop_block" });
        for index in 0..2 {
            assert_eq!(requests[index]["thinking"].get("block_binding"), None);
            assert!(!headers[index].contains("thinking-binding-controls-2026-08-01"));
        }
        for index in 2..5 {
            assert_eq!(requests[index]["thinking"]["block_binding"], drop_block);
            assert!(
                headers[index].contains(
                    "anthropic-beta: oauth-2025-04-20,thinking-binding-controls-2026-08-01"
                ),
                "{}",
                headers[index]
            );
        }
    }

    #[tokio::test]
    async fn saves_finished_tool_rounds_while_the_prompt_runs() {
        let api = MockApi::start(vec![
            tool_reply("call-1", "bash", json!({ "command": "echo round-one" })),
            Reply::Stall,
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let session_file = test_support::session_file(&agent);
        let mut prompt = Box::pin(agent.prompt("run it", &[], |_| {}));
        let second_request_sent = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _ = &mut prompt => return false,
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if api.requests().await.len() == 2 {
                            return true;
                        }
                    }
                }
            }
        })
        .await;
        let saved = std::fs::read_to_string(&session_file).unwrap_or_default();
        drop(prompt);
        test_support::remove_session(&agent);
        assert_eq!(second_request_sent, Ok(true));
        assert!(
            saved.contains("tool_result") && saved.contains("round-one"),
            "{saved}"
        );
    }

    #[tokio::test]
    async fn keeps_finished_tool_calls_when_cancelled_during_a_later_call() {
        let two_tool_calls = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call-1", "name": "bash", "input": { "command": "echo first-done" } } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "call-2", "name": "bash", "input": { "command": "sleep 30" } } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }),
        ]);
        let api = MockApi::start(vec![two_tool_calls, text_reply("ok")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let session_file = test_support::session_file(&agent);
        let finished = std::cell::Cell::new(0);
        let mut prompt = Box::pin(agent.prompt("run both", &[], |event| {
            if let AgentEvent::ToolDone { .. } = event {
                finished.set(finished.get() + 1);
            }
        }));
        // The second call sleeps for 30 seconds, so it is still running once
        // the first call is done.
        let first_call_finished = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _ = &mut prompt => return false,
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if finished.get() == 1 {
                            return true;
                        }
                    }
                }
            }
        })
        .await;
        drop(prompt);
        let cancelled = agent.cancel(0);
        let saved = std::fs::read_to_string(&session_file).unwrap_or_default();
        let next = agent.prompt("next", &[], |_| {}).await;
        test_support::remove_session(&agent);
        assert_eq!(first_call_finished, Ok(true));
        assert_eq!(finished.get(), 1);
        cancelled.unwrap();
        next.unwrap();
        assert!(saved.contains("first-done"), "{saved}");
        let requests = api.requests().await;
        let results: Vec<_> = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .filter(|block| block["type"] == "tool_result")
            .map(|block| (block["tool_use_id"].clone(), block["is_error"].clone()))
            .collect();
        assert_eq!(
            results,
            vec![
                (json!("call-1"), json!(false)),
                (json!("call-2"), json!(true))
            ]
        );
        assert!(requests[1]["messages"].to_string().contains("first-done"));
    }

    #[tokio::test]
    async fn keeps_queued_prompts_word_for_word_when_compacting_after_a_tool_round() {
        let full_context_tool_call = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 100_000_000, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call-1", "name": "bash", "input": { "command": "true" } } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }),
        ]);
        let api = MockApi::start(vec![
            full_context_tool_call,
            text_reply("summary of the work"),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        agent.queue.lock().unwrap().push(Queued {
            prompt: "also update the README".into(),
            images: Vec::new(),
        });
        let result = agent.prompt("fix the bug", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 3);
        let after_compaction = requests[2]["messages"].to_string();
        assert!(
            after_compaction.contains("summary of the work"),
            "{after_compaction}"
        );
        assert!(
            after_compaction.contains("also update the README"),
            "{after_compaction}"
        );
    }

    #[tokio::test]
    async fn compacts_a_conversation_that_outgrew_the_context_window() {
        let full_context_tool_call = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 100_000_000, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call-1", "name": "bash", "input": { "command": "head -c 15000 /dev/zero | tr '\\0' x" } } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }),
        ]);
        // The tool output alone is larger than the API accepts.
        let api = MockApi::start_with_message_limit(
            vec![
                full_context_tool_call,
                text_reply("summary of the work"),
                text_reply("done"),
            ],
            10_000,
        )
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut notices = Vec::new();
        let result = agent
            .prompt("read the big file", &[], |event| {
                if let AgentEvent::Notice(notice) = event {
                    notices.push(notice);
                }
            })
            .await;
        test_support::remove_session(&agent);
        result.unwrap();
        assert!(
            notices.iter().any(|notice| notice.contains("too long")),
            "{notices:?}"
        );
        let requests = api.requests().await;
        let last = requests.last().unwrap()["messages"].to_string();
        assert!(last.contains("summary of the work"), "{last}");
        // The session keeps the full tool output.
        let output = "x".repeat(15_000);
        assert!(
            serde_json::Value::from(agent.messages())
                .to_string()
                .contains(&output)
        );
    }

    #[tokio::test]
    async fn lets_prompts_be_queued_while_queued_images_are_saved() {
        use std::sync::{
            TryLockError,
            atomic::{AtomicBool, Ordering},
        };

        let api = MockApi::start(vec![
            tool_reply("call-1", "bash", json!({ "command": "true" })),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let small = Image {
            media_type: "image/png",
            data: vec![1; 16],
        };
        // Large enough that saving it takes a while.
        let large = Image {
            media_type: "image/png",
            data: vec![2; 8 << 20],
        };
        agent.queue.lock().unwrap().push(Queued {
            prompt: "look at these".into(),
            images: vec![(1, small), (2, large)],
        });
        let directory = test_support::session_file(&agent).with_file_name(&agent.session.id);
        let queue = agent.queue.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let watching = finished.clone();
        // Watches the queue while the first image is saved and the second
        // one is being saved.
        let watcher = std::thread::spawn(move || {
            let mut blocked = false;
            while !watching.load(Ordering::Relaxed) {
                let saved = std::fs::read_dir(&directory).map_or(0, |entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter(|entry| entry.path().extension() == Some("png".as_ref()))
                        .count()
                });
                if saved == 1 && matches!(queue.try_lock(), Err(TryLockError::WouldBlock)) {
                    blocked = true;
                }
            }
            blocked
        });
        let result = agent.prompt("run it", &[], |_| {}).await;
        finished.store(true, Ordering::Relaxed);
        let blocked = watcher.join().unwrap();
        test_support::remove_session(&agent);
        result.unwrap();
        assert!(!blocked, "the queue was locked while an image was saved");
        assert!(agent.queue.lock().unwrap().is_empty());
        let requests = api.requests().await;
        assert!(
            requests[1]["messages"]
                .to_string()
                .contains("look at these")
        );
    }

    #[tokio::test]
    async fn keeps_prompts_queued_when_their_image_cannot_be_saved() {
        let api = MockApi::start(vec![
            tool_reply("call-1", "bash", json!({ "command": "true" })),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let image = Image {
            media_type: "image/png",
            data: vec![1; 16],
        };
        agent.queue.lock().unwrap().push(Queued {
            prompt: "look at this".into(),
            images: vec![(1, image)],
        });
        // A file in the way of the session's image folder makes saving fail.
        let images = test_support::session_file(&agent).with_file_name(&agent.session.id);
        std::fs::create_dir_all(images.parent().unwrap()).unwrap();
        std::fs::write(&images, "").unwrap();
        let result = agent.prompt("run it", &[], |_| {}).await;
        let _ = std::fs::remove_file(&images);
        test_support::remove_session(&agent);
        assert!(result.is_err());
        let queued: Vec<_> = agent
            .queue
            .lock()
            .unwrap()
            .iter()
            .map(|queued| queued.prompt.clone())
            .collect();
        assert_eq!(queued, ["look at this"]);
    }

    #[tokio::test]
    async fn keeps_the_conversation_cached_when_a_queued_prompt_follows_a_tool_round() {
        let api = MockApi::start(vec![
            tool_reply("call-1", "bash", json!({ "command": "true" })),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        agent.queue.lock().unwrap().push(Queued {
            prompt: "also this".into(),
            images: Vec::new(),
        });
        let result = agent.prompt("run it", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 2);
        let messages = requests[1]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        let marked: Vec<bool> = messages
            .iter()
            .map(|message| message.to_string().contains("cache_control"))
            .collect();
        // The prompt ended the first request, where its cache entry was written.
        assert_eq!(marked, [true, false, false, true]);
    }

    #[tokio::test]
    async fn keeps_the_conversation_cached_after_a_round_with_many_tool_calls() {
        let mut events = vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "plan", "signature": "sig" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "running" } }),
            json!({ "type": "content_block_stop", "index": 1 }),
        ];
        for index in 2..12 {
            events.push(json!({ "type": "content_block_start", "index": index, "content_block": { "type": "tool_use", "id": format!("call-{index}"), "name": "bash", "input": { "command": "true" } } }));
            events.push(json!({ "type": "content_block_stop", "index": index }));
        }
        events.push(json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }));
        let api = MockApi::start(vec![Reply::Events(events), text_reply("done")]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let result = agent.prompt("run them all", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 2);
        let body = &requests[1];
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        let breakpoint = |message: &serde_json::Value| match message["content"].as_array() {
            Some(blocks) => blocks.last().unwrap()["cache_control"].clone(),
            None => serde_json::Value::Null,
        };
        let ephemeral = json!({ "type": "ephemeral" });
        // The prompt ended the first request, where its cache entry was written.
        assert_eq!(breakpoint(&messages[0]), ephemeral);
        assert_eq!(breakpoint(&messages[2]), ephemeral);
        assert!(
            body.to_string().matches("cache_control").count() <= 4,
            "{body}"
        );
        for block in messages[1]["content"].as_array().unwrap() {
            if block["type"] == "thinking" {
                assert_eq!(block.get("cache_control"), None);
            }
        }
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
