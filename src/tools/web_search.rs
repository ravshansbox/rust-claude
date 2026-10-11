use serde_json::{Value, json};
use std::fmt::Write;

pub const NAME: &str = "web_search";

pub fn definition() -> Value {
    json!({ "type": "web_search_20250305", "name": NAME })
}

/// The number of results a search found, or the error code it failed with.
pub fn outcome(content: &Value) -> Result<String, String> {
    match content.as_array() {
        Some(results) if results.len() == 1 => Ok("1 result".into()),
        Some(results) => Ok(format!("{} results", results.len())),
        None => Err(content["error_code"]
            .as_str()
            .unwrap_or("unknown error")
            .into()),
    }
}

/// The pages cited in the text blocks of a reply, listed once each.
pub fn sources(content: &[Value]) -> Option<String> {
    let mut urls = Vec::new();
    let mut list = String::new();
    let citations = content
        .iter()
        .filter(|block| block["type"] == "text")
        .flat_map(|block| block["citations"].as_array().into_iter().flatten());
    for citation in citations {
        let Some(url) = citation["url"].as_str() else {
            continue;
        };
        if urls.contains(&url) {
            continue;
        }
        urls.push(url);
        let title = citation["title"].as_str().unwrap_or(url);
        let _ = writeln!(list, "- {title}: {url}");
    }
    (!list.is_empty()).then(|| format!("\n\nSources:\n{list}"))
}
