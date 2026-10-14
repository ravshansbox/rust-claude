use std::fmt::Write;

use serde_json::{Value, json};

pub(super) const MAX_TOOL_NAME: usize = 64;

pub(super) struct Tool {
    pub(super) name: String,
    pub(super) qualified_name: String,
    pub(super) definition: Value,
}

pub(super) fn qualified_name(server: &str, tool: &str) -> String {
    ["mcp__", server, "__", tool]
        .into_iter()
        .flat_map(str::chars)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(MAX_TOOL_NAME)
        .collect()
}

pub(super) fn tool_definition(qualified_name: &str, mut tool: Value) -> Value {
    let mut schema = match tool["inputSchema"].take() {
        Value::Object(schema) => schema,
        _ => serde_json::Map::new(),
    };
    for key in ["anyOf", "oneOf", "allOf"] {
        schema.remove(key);
    }
    schema.insert("type".into(), json!("object"));
    if !schema.get("properties").is_some_and(Value::is_object) {
        schema.insert("properties".into(), json!({}));
    }
    let description = tool["description"]
        .as_str()
        .or_else(|| tool["title"].as_str())
        .or_else(|| tool["name"].as_str())
        .unwrap_or_default();
    json!({
        "name": qualified_name,
        "description": description,
        "input_schema": schema,
    })
}

pub(super) fn result_text(result: &Value) -> String {
    let blocks = result["content"].as_array().map_or(&[][..], Vec::as_slice);
    if blocks.is_empty() && !result["structuredContent"].is_null() {
        return result["structuredContent"].to_string();
    }
    let mut text = String::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let mime_type = block["mimeType"].as_str().unwrap_or("unknown type");
        let _ = match block["type"].as_str() {
            Some("text") => text.write_str(block["text"].as_str().unwrap_or_default()),
            Some("image") => write!(text, "[image: {mime_type}]"),
            Some("audio") => write!(text, "[audio: {mime_type}]"),
            Some("resource_link") => write!(
                text,
                "[resource link: {}]",
                block["uri"].as_str().unwrap_or("unknown")
            ),
            Some("resource") => match block["resource"]["text"].as_str() {
                Some(resource_text) => text.write_str(resource_text),
                None => write!(
                    text,
                    "[resource: {}]",
                    block["resource"]["uri"].as_str().unwrap_or("unknown")
                ),
            },
            _ => write!(text, "{block}"),
        };
    }
    text
}
