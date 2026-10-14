use anyhow::Result;
use serde_json::{Value, json};

use super::{Agent, context::is_compaction, usage::total_usage};
use crate::{images::Image, skills};

/// The result saved for a tool call that was cancelled before it finished.
pub const CANCELLED: &str = "cancelled by the user";

pub(super) fn cancel_point(messages: &[Value], checkpoint: usize) -> usize {
    messages[checkpoint..]
        .iter()
        .rposition(|message| message["role"] == "user" || is_compaction(message))
        .map_or(checkpoint, |index| checkpoint + index + 1)
}

impl Agent {
    pub(super) fn discard_from(&mut self, index: usize, stop_reason: &str, always_mark: bool) {
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
    pub(super) fn push_tool_results(&mut self) {
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
}
