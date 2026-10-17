use anyhow::Result;
use serde_json::Value;

use super::{
    Agent, AgentEvent,
    retry::{MAX_RETRIES, is_thinking_mismatch, retry_delay},
    stream::Response,
    subagent::remove_session_ids,
};

impl Agent {
    pub(super) async fn request(
        &mut self,
        mut messages: Vec<Value>,
        tool_choice: Option<Value>,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Response> {
        self.session.inline_images(&mut messages)?;
        remove_session_ids(&mut messages);
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
}
