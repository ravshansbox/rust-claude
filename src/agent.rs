use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

mod cancel;
mod compact;
mod context;
mod event;
mod http;
mod queue;
mod request;
pub(crate) mod retry;
mod shell;
mod stats;
mod stream;
mod subagent;
mod tool_call;
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
pub use cancel::CANCELLED;
use cancel::cancel_point;
use context::{active_messages, active_start, has_uncompacted, with_cache_breakpoint};
pub use event::AgentEvent;
pub use http::http_client;
pub use queue::{Queue, Queued, take_queued};
use retry::is_prompt_too_long;
pub use shell::{parse_shell_message, shell_message};
use stream::Response;
pub use system::Instructions;
use system::load_instructions;
pub use usage::{Quota, Stats, Usage};

const API_BASE: &str = "https://api.anthropic.com";
const OAUTH_BETA: &str = "oauth-2025-04-20";

fn text_of(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect()
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
    pub(crate) background: tools::background::Background,
}

impl Agent {
    pub fn new(http: reqwest::Client, credentials: Credentials, model: String) -> Result<Self> {
        Ok(Self {
            instructions: load_instructions(),
            skills: skills::load(),
            ..Self::base(http, credentials, model)?
        })
    }

    fn base(http: reqwest::Client, credentials: Credentials, model: String) -> Result<Self> {
        Ok(Self {
            http,
            api_base: API_BASE.into(),
            credentials,
            model,
            effort: models::DEFAULT_EFFORT,
            messages: Vec::new(),
            pending_usage: Usage::default(),
            tool_results: Vec::new(),
            quota: Quota::default(),
            instructions: Vec::new(),
            skills: Skills::default(),
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

    pub fn new_session(&mut self) -> Result<()> {
        self.session = Session::new()?;
        self.messages.clear();
        self.background = tools::background::Background::default();
        Ok(())
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
            let messages = with_cache_breakpoint(active_messages(&self.messages));
            let response = match self.request(messages, None, &mut on_event).await {
                Err(error)
                    if is_prompt_too_long(&error) && has_uncompacted(&self.messages[..end]) =>
                {
                    self.compact_history(end, &mut on_event)
                        .await
                        .map_err(|compact_error| anyhow!("{error:#}; {compact_error:#}"))?;
                    // The summary now sits at `end`, so it belongs to the
                    // compacted part like the messages it replaced.
                    end += 1;
                    let messages = with_cache_breakpoint(active_messages(&self.messages));
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
            if let Some(sources) =
                tools::sources::sources(&self.messages[active_start(&self.messages)..], &content)
            {
                on_event(AgentEvent::Text(sources));
            }
            if stop_reason == "pause_turn" {
                continue;
            }
            if stop_reason != "tool_use" {
                return Ok(());
            }

            for block in content.iter().filter(|block| block["type"] == "tool_use") {
                self.call_tool(block, &mut on_event).await;
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
}

#[cfg(test)]
mod tests;
