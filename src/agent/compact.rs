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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{
        Queued,
        test_support::{self, MockApi, Reply, stopped_reply, text_reply, tool_reply},
    };

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
    async fn answers_the_prompt_when_automatic_compaction_fails() {
        let full_context_reply = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 100_000_000, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "hello" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 1 } }),
            json!({ "type": "message_stop" }),
        ]);
        let api = MockApi::start(vec![
            full_context_reply,
            stopped_reply("partial summary", "refusal"),
            text_reply("ok"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut notices = Vec::new();
        let first = agent.prompt("hi", &[], |_| {}).await;
        let next = agent
            .prompt("next", &[], test_support::notices(&mut notices))
            .await;
        test_support::remove_session(&agent);
        first.unwrap();
        next.unwrap();
        assert!(
            notices
                .iter()
                .any(|notice| notice.contains("not compacted")),
            "{notices:?}"
        );
        let requests = api.requests().await;
        let last = requests[2]["messages"].to_string();
        assert!(last.contains("hello"), "{last}");
        assert!(!last.contains("partial summary"), "{last}");
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
    async fn does_not_summarise_again_when_the_request_after_compaction_is_too_long() {
        let full_context_tool_call = Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 100_000_000, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call-1", "name": "bash", "input": { "command": "true" } } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 1 } }),
        ]);
        let api = MockApi::start_with_message_limit(
            vec![
                full_context_tool_call,
                text_reply("summary of the work"),
                text_reply("second summary"),
            ],
            10_000,
        )
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        agent.queue.lock().unwrap().push(Queued {
            prompt: "x".repeat(15_000),
            images: Vec::new(),
        });
        let mut notices = Vec::new();
        let result = agent
            .prompt("fix the bug", &[], test_support::notices(&mut notices))
            .await;
        test_support::remove_session(&agent);
        let message = result.unwrap_err().to_string();
        assert!(message.contains("prompt is too long"), "{message}");
        let compactions = notices
            .iter()
            .filter(|notice| *notice == "compacting conversation")
            .count();
        assert_eq!(compactions, 1, "{notices:?}");
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
        let mut notices = Vec::new();
        let (agent, result) = test_support::run_prompt(
            &api,
            "read the big file",
            test_support::notices(&mut notices),
        )
        .await;
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
    async fn compacts_when_tool_output_makes_the_next_request_too_long() {
        let api = MockApi::start_with_message_limit(
            vec![
                tool_reply(
                    "call-1",
                    "bash",
                    json!({ "command": "head -c 15000 /dev/zero | tr '\\0' x" }),
                ),
                text_reply("summary of the work"),
                text_reply("done"),
            ],
            10_000,
        )
        .await;
        let (_, result) = test_support::run_prompt(&api, "read the big file", |_| {}).await;
        result.unwrap();
        let requests = api.requests().await;
        let last = requests.last().unwrap()["messages"].to_string();
        assert!(last.contains("summary of the work"), "{last}");
    }
}
