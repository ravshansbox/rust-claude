use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::{
    Agent, AgentEvent, Usage,
    context::{active_messages, has_uncompacted, shorten_tool_results, with_cache_breakpoint},
    retry::is_prompt_too_long,
    stream::Response,
    text_of,
};

const COMPACT_PROMPT: &str = "Summarise this conversation so that you can continue the work from the summary alone. Include the user's requests, decisions made, files read and changed, the current state of the work and the next steps. Do not call tools. Reply with the summary only.";
const COMPACT_AT_PERCENT: u64 = 80;
/// How many characters of each tool output to keep, in turn, when the
/// conversation is too long to summarise.
const COMPACT_TOOL_RESULT_LIMITS: [usize; 2] = [2_000, 200];

impl Agent {
    pub async fn compact(&mut self, mut on_event: impl FnMut(AgentEvent)) -> Result<()> {
        let end = self.messages.len();
        let result = self.compact_history(end, &mut on_event).await;
        if let Err(error) = result {
            self.discard_from(self.messages.len(), "error", false);
            return Err(self.save_after_failure(error));
        }
        self.session.save(&self.messages)
    }

    pub(super) fn context_full(&self) -> bool {
        let stats = self.stats();
        stats.context_window > 0
            && stats
                .context_tokens
                .is_some_and(|tokens| tokens * 100 >= stats.context_window * COMPACT_AT_PERCENT)
    }

    pub(super) async fn compact_if_full(
        &mut self,
        end: usize,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> usize {
        if !self.context_full() || !has_uncompacted(&self.messages[..end]) {
            return end;
        }
        match self.compact_history(end, on_event).await {
            Ok(()) => end + 1,
            Err(error) => {
                on_event(AgentEvent::Notice(format!("{error:#}")));
                end
            }
        }
    }

    pub(super) async fn compact_history(
        &mut self,
        end: usize,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<()> {
        if !has_uncompacted(&self.messages[..end]) {
            bail!("nothing to compact");
        }
        on_event(AgentEvent::Notice("compacting conversation".into()));
        let active = active_messages(&self.messages[..end]);
        let mut response = self.summarise(active.clone(), on_event).await;
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
                .summarise(shorten_tool_results(&active, limit), on_event)
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
        let summary = text_of(&response.content);
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
        messages: Vec<Value>,
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
}

#[cfg(test)]
mod tests;
