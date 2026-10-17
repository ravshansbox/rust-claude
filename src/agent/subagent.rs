use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::sync::mpsc;

mod child;

use super::{Agent, AgentEvent};
use crate::tools::agent::{self, Mode};

/// A task's subagent session id, if it started, and its reply.
type TaskReply = (Option<String>, Result<String, String>);

const NOT_RUN: &str = "This task did not run, because an earlier task failed.";
const FAILED: &str = "This task failed: ";
const NO_REPLY: &str = "This task finished without a reply.";

pub(crate) fn is_failure(reply: &str) -> bool {
    reply.starts_with(FAILED) || reply == NOT_RUN
}

impl Agent {
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
                        let prompt = match replies.last().map(|(_, reply)| reply) {
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
                .map(|(session, reply)| {
                    let mut block =
                        json!({ "type": "text", "text": reply.unwrap_or_else(|error| error) });
                    if let Some(session) = session {
                        block["session"] = json!(session);
                    }
                    block
                })
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
    ) -> Pin<Box<dyn Future<Output = TaskReply> + Send + '_>> {
        Box::pin(async move {
            let _ = events.send(AgentEvent::TaskStart { task, description });
            let tool_calls = Arc::new(AtomicUsize::new(0));
            let (session, reply) = match prompt {
                Some(prompt) => {
                    self.run_subagent(task, prompt, events.clone(), tool_calls.clone())
                        .await
                }
                None => (None, Err(NOT_RUN.into())),
            };
            let _ = events.send(AgentEvent::TaskDone {
                task,
                reply: reply.clone(),
                tool_calls: tool_calls.load(Ordering::Relaxed),
            });
            (session, reply)
        })
    }
}

/// Removes the subagent session ids saved with task replies, which the API
/// does not accept.
pub(super) fn remove_session_ids(messages: &mut [Value]) {
    let blocks = messages
        .iter_mut()
        .filter_map(|message| message["content"].as_array_mut())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| block["content"].as_array_mut())
        .flatten();
    for block in blocks {
        if let Some(block) = block.as_object_mut() {
            block.remove("session");
        }
    }
}

#[cfg(test)]
mod tests;
