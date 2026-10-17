use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{FAILED, NO_REPLY, TaskReply};
use crate::{
    agent::{Agent, AgentEvent, Usage, text_of},
    session::Session,
    skills::Skills,
};

type Callback = Box<dyn FnMut(AgentEvent) + Send>;

impl Agent {
    fn subagent(&self) -> Result<Self> {
        Ok(Self {
            api_base: self.api_base.clone(),
            effort: self.effort,
            quota: self.quota,
            instructions: self.instructions.clone(),
            skills: Skills {
                skills: self.skills.skills.clone(),
                diagnostics: Vec::new(),
            },
            mcp: self.mcp.clone(),
            session: Session::subagent(&self.session.id)?,
            subagent: true,
            ..Self::base(
                self.http.clone(),
                self.credentials.clone(),
                self.model.clone(),
            )?
        })
    }

    pub(super) async fn run_subagent(
        &self,
        task: usize,
        prompt: String,
        events: mpsc::UnboundedSender<AgentEvent>,
        tool_calls: Arc<AtomicUsize>,
    ) -> TaskReply {
        let failed = |error: anyhow::Error| format!("{FAILED}{error:#}");
        let mut child = match self.subagent() {
            Ok(child) => child,
            Err(error) => return (None, Err(failed(error))),
        };
        let id = Some(child.session.id.clone());
        let usage = self.subagent_usage.clone();
        let slot = usage.lock().map_or(0, |mut slots| {
            slots.push(Usage::default());
            slots.len() - 1
        });
        let count = move |counted: Usage| {
            if let Ok(mut slots) = usage.lock()
                && let Some(slot) = slots.get_mut(slot)
            {
                *slot = counted;
            }
        };
        let count_failed = count.clone();
        let callback: Callback = Box::new(move |event| match &event {
            AgentEvent::Stats(stats) => {
                count(stats.usage);
                let _ = events.send(AgentEvent::TaskStats {
                    task,
                    stats: *stats,
                });
                let _ = events.send(event);
            }
            AgentEvent::ToolStart { .. } => {
                let count = tool_calls.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = events.send(AgentEvent::TaskEvent {
                    task,
                    event: Box::new(event),
                });
                let _ = events.send(AgentEvent::TaskToolCalls { task, count });
            }
            AgentEvent::Text(_)
            | AgentEvent::Thinking(_)
            | AgentEvent::ToolDone { .. }
            | AgentEvent::Notice(_) => {
                let _ = events.send(AgentEvent::TaskEvent {
                    task,
                    event: Box::new(event),
                });
            }
            _ => {}
        });
        if let Err(error) = child.run(&prompt, &[], callback).await {
            child.discard_from(child.messages.len(), "error", false);
            count_failed(child.stats().usage);
            let error = child.save_after_failure(error);
            return (id, Err(failed(error)));
        }
        let reply = final_reply(&child.messages);
        match child.session.save(&child.messages) {
            Ok(()) => (id, Ok(reply)),
            Err(error) => (
                None,
                Ok(format!(
                    "{reply}\n\nfailed to save this conversation: {error:#}"
                )),
            ),
        }
    }
}

fn final_reply(messages: &[Value]) -> String {
    let start = messages
        .iter()
        .rposition(|message| message["role"] == "user")
        .map_or(0, |index| index + 1);
    let reply: String = messages[start..]
        .iter()
        .filter(|message| message["role"] == "assistant" && message.get("stop_reason").is_none())
        .filter_map(|message| message["content"].as_array())
        .map(|content| text_of(content))
        .collect();
    if reply.trim().is_empty() {
        NO_REPLY.to_string()
    } else {
        reply
    }
}
