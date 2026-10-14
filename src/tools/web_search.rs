use serde_json::{Value, json};

pub const NAME: &str = "web_search";

pub fn definition() -> Value {
    json!({ "type": "web_search_20250305", "name": NAME })
}

/// The number of results a search found, or the error code it failed with.
pub fn outcome(content: &Value) -> Result<String, String> {
    match content.as_array() {
        Some(results) => Ok(super::plural(results.len(), "result")),
        None => Err(content["error_code"]
            .as_str()
            .unwrap_or("unknown error")
            .into()),
    }
}
