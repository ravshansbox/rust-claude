use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

mod cancel;
mod compact;
mod context;
mod event;
mod http;
mod queue;
mod request;
pub(crate) mod retry;
mod sessions;
mod shell;
mod stats;
mod stream;
mod subagent;
mod tool_call;
mod turn;
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
    session::Session,
    skills::{self, Skills},
};
pub use cancel::CANCELLED;
use cancel::cancel_point;
pub use event::AgentEvent;
pub use http::http_client;
pub use queue::{Queue, Queued, take_queued};
pub use shell::parse_shell_message;
#[cfg(test)]
pub use shell::shell_message;
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
    /// start subagents, and its conversation is not saved.
    subagent: bool,
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
        })
    }

    pub fn messages(&self) -> &[Value] {
        &self.messages
    }

    pub fn take_renewal_notice(&mut self) -> Option<String> {
        self.credentials.take_renewal_notice()
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
}

#[cfg(test)]
mod tests;
