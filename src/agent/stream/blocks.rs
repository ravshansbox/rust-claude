use crate::{agent::AgentEvent, tools};
use serde_json::{Value, json};

pub(super) fn start_block(
    content: &mut Vec<Value>,
    block: Value,
    on_event: &mut impl FnMut(AgentEvent),
) {
    let result = match block["type"].as_str() {
        Some("web_search_tool_result") => Some((
            tools::web_search::NAME,
            tools::web_search::outcome(&block["content"]),
        )),
        Some("web_fetch_tool_result") => Some((
            tools::web_fetch::NAME,
            tools::web_fetch::outcome(&block["content"]),
        )),
        _ => None,
    };
    if let Some((name, outcome)) = result {
        on_event(AgentEvent::ToolDone {
            name: name.into(),
            note: outcome.as_ref().ok().cloned(),
            error: outcome.err(),
        });
    }
    content.push(block);
}

pub(super) fn apply_delta(
    content: &mut [Value],
    partial_json: &mut String,
    delta: &mut Value,
    on_event: &mut impl FnMut(AgentEvent),
) {
    let Some(block) = content.last_mut() else {
        return;
    };
    let key = match delta["type"].as_str().unwrap_or_default() {
        "text_delta" => "text",
        "thinking_delta" => "thinking",
        "signature_delta" => "signature",
        "citations_delta" => {
            let citation = delta["citation"].take();
            match block.get_mut("citations") {
                Some(Value::Array(citations)) => citations.push(citation),
                _ => block["citations"] = Value::Array(vec![citation]),
            }
            return;
        }
        "input_json_delta" => {
            partial_json.push_str(delta["partial_json"].as_str().unwrap_or_default());
            return;
        }
        _ => return,
    };
    let addition = delta[key].as_str().unwrap_or_default();
    match block.get_mut(key) {
        Some(Value::String(text)) => text.push_str(addition),
        _ => block[key] = json!(addition),
    }
    match key {
        "text" => on_event(AgentEvent::Text(addition.into())),
        "thinking" => on_event(AgentEvent::Thinking(addition.into())),
        _ => {}
    }
}

pub(super) fn finish_block(
    content: &mut [Value],
    partial_json: &str,
    input_error: &mut Option<serde_json::Error>,
    on_event: &mut impl FnMut(AgentEvent),
) {
    if let Some(block) = content.last_mut()
        && (block["type"] == "tool_use" || block["type"] == "server_tool_use")
        && !partial_json.is_empty()
    {
        match serde_json::from_str(partial_json) {
            Ok(input) => block["input"] = input,
            Err(error) => *input_error = Some(error),
        }
    }
    if let Some(block) = content.last()
        && block["type"] == "server_tool_use"
    {
        let name = block["name"].as_str().unwrap_or_default();
        on_event(AgentEvent::ToolStart {
            name: name.into(),
            summary: tools::summary(name, &block["input"]),
            diff: None,
        });
    }
}
