use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use super::{
    Agent, AgentEvent, Usage,
    context::{active_messages, active_start, has_uncompacted, with_cache_breakpoint},
    retry::is_prompt_too_long,
    stream::Response,
};
use crate::{images::Image, tools};

impl Agent {
    pub(super) fn push_prompt(&mut self, prompt: &str, images: &[Image]) -> Result<()> {
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

    pub(super) async fn run(
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
