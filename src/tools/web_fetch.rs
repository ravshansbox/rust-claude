use serde_json::{Value, json};

pub const NAME: &str = "web_fetch";

pub fn definition() -> Value {
    json!({ "type": "web_fetch_20250910", "name": NAME, "citations": { "enabled": true } })
}

/// The title of the page fetched, or its URL when it has none, or the
/// error code the fetch failed with.
pub fn outcome(content: &Value) -> Result<String, String> {
    if content["type"] != "web_fetch_result" {
        return Err(content["error_code"]
            .as_str()
            .unwrap_or("unknown error")
            .into());
    }
    let url = content["url"].as_str().unwrap_or_default();
    let title = content["content"]["title"].as_str();
    Ok(title
        .filter(|title| !title.is_empty())
        .unwrap_or(url)
        .into())
}

/// The pages fetched in `messages`, in the order the API numbers them in
/// citations, each as its URL and title.
pub fn pages(messages: &[Value]) -> Vec<(&str, Option<&str>)> {
    messages
        .iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "web_fetch_tool_result")
        .map(|block| &block["content"])
        .filter(|content| content["type"] == "web_fetch_result")
        .map(|content| {
            (
                content["url"].as_str().unwrap_or_default(),
                content["content"]["title"].as_str(),
            )
        })
        .collect()
}
