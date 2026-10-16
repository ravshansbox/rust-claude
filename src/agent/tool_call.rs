use serde_json::{Value, json};

use super::{Agent, AgentEvent};
use crate::tools;

impl Agent {
    pub(super) fn tool_definitions(&self) -> Value {
        let mut definitions = tools::definitions();
        if let Value::Array(list) = &mut definitions {
            if self.subagent {
                list.retain(|definition| definition["name"] != tools::agent::NAME);
            }
            list.extend(self.mcp.definitions());
        }
        definitions
    }

    /// Runs a tool call from a reply and keeps its result for the next request.
    pub(super) async fn call_tool(&mut self, block: &Value, on_event: &mut impl FnMut(AgentEvent)) {
        let name = block["name"].as_str().unwrap_or_default();
        on_event(AgentEvent::ToolStart {
            name: name.into(),
            summary: tools::summary(name, &block["input"]),
            diff: tools::diff(name, &block["input"]),
        });
        let result = if name == tools::agent::NAME && !self.subagent {
            self.run_tasks(&block["input"], on_event).await
        } else {
            match self.mcp.call(name, &block["input"]).await {
                Some(result) => result.map(Value::from),
                None => match tools::run(name, &block["input"]).await {
                    Ok(tools::Output::Text(text)) => Ok(Value::from(text)),
                    Ok(tools::Output::Image(image)) => self
                        .session
                        .save_image(&image)
                        .map(|image| json!([image]))
                        .map_err(|error| format!("{error:#}")),
                    Err(error) => Err(error),
                },
            }
        };
        let (content, is_error) = match result {
            Ok(content) => (content, false),
            Err(text) => (Value::from(text), true),
        };
        let text = content.as_str().unwrap_or_default();
        on_event(AgentEvent::ToolDone {
            name: name.into(),
            error: is_error.then(|| text.to_string()),
            note: tools::note(name, text).filter(|_| !is_error),
        });
        for notice in self.mcp.take_renewal_notices() {
            on_event(AgentEvent::Notice(notice));
        }
        self.tool_results.push(json!({
            "type": "tool_result",
            "tool_use_id": block["id"],
            "content": content,
            "is_error": is_error,
        }));
    }
}
