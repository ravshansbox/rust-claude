use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{Agent, AgentEvent, Usage, text_of};
use crate::{
    skills::Skills,
    tools::agent::{self, Mode},
};

const NOT_RUN: &str = "This task did not run, because an earlier task failed.";
const FAILED: &str = "This task failed: ";
const NO_REPLY: &str = "This task finished without a reply.";

pub(crate) fn is_failure(reply: &str) -> bool {
    reply.starts_with(FAILED) || reply == NOT_RUN
}

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
            subagent: true,
            ..Self::base(
                self.http.clone(),
                self.credentials.clone(),
                self.model.clone(),
            )?
        })
    }

    /// Runs the tasks of an `agent` tool call, each in its own subagent, and
    /// gives one text block per task with its final reply.
    pub(super) async fn run_tasks(
        &self,
        input: &Value,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Value, String> {
        let (mode, tasks) = agent::parse(input)?;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let work = async move {
            match mode {
                Mode::Parallel => {
                    futures::future::join_all(tasks.into_iter().enumerate().map(|(index, task)| {
                        self.run_task(index, task.description, Some(task.prompt), sender.clone())
                    }))
                    .await
                }
                Mode::Sequential => {
                    let mut replies = Vec::new();
                    for (index, task) in tasks.into_iter().enumerate() {
                        let prompt = match replies.last() {
                            Some(Err(_)) => None,
                            Some(Ok(previous)) => Some(format!(
                                "{}\n\n<previous-task-result>\n{previous}\n</previous-task-result>",
                                task.prompt
                            )),
                            None => Some(task.prompt),
                        };
                        let reply = self
                            .run_task(index, task.description, prompt, sender.clone())
                            .await;
                        replies.push(reply);
                    }
                    replies
                }
            }
        };
        let forward = async {
            while let Some(event) = receiver.recv().await {
                match event {
                    AgentEvent::Stats(_) => on_event(AgentEvent::Stats(self.stats())),
                    event => on_event(event),
                }
            }
        };
        let (replies, ()) = futures::join!(work, forward);
        Ok(Value::Array(
            replies
                .into_iter()
                .map(|reply| json!({ "type": "text", "text": reply.unwrap_or_else(|error| error) }))
                .collect(),
        ))
    }

    /// Boxed, because a subagent's run holds this future's type in turn.
    fn run_task(
        &self,
        task: usize,
        description: String,
        prompt: Option<String>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async move {
            let _ = events.send(AgentEvent::TaskStart { task, description });
            let tool_calls = Arc::new(AtomicUsize::new(0));
            let reply = match prompt {
                Some(prompt) => {
                    self.run_subagent(task, prompt, events.clone(), tool_calls.clone())
                        .await
                }
                None => Err(NOT_RUN.into()),
            };
            let _ = events.send(AgentEvent::TaskDone {
                task,
                reply: reply.clone(),
                tool_calls: tool_calls.load(Ordering::Relaxed),
            });
            reply
        })
    }

    async fn run_subagent(
        &self,
        task: usize,
        prompt: String,
        events: mpsc::UnboundedSender<AgentEvent>,
        tool_calls: Arc<AtomicUsize>,
    ) -> Result<String, String> {
        let failed = |error: anyhow::Error| format!("{FAILED}{error:#}");
        let mut child = self.subagent().map_err(failed)?;
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
                let _ = events.send(AgentEvent::TaskToolCalls { task, count });
            }
            _ => {}
        });
        if let Err(error) = child.run(&prompt, &[], callback).await {
            child.discard_from(child.messages.len(), "error", false);
            count_failed(child.stats().usage);
            return Err(failed(error));
        }
        Ok(final_reply(&child.messages))
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

#[cfg(test)]
mod tests;
