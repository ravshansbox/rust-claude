use serde_json::Value;
use std::fmt::Write;

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
