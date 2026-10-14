use super::web_fetch;
use serde_json::Value;
use std::fmt::Write;

/// The pages cited in the text blocks of a reply, listed once each.
/// `context` holds the messages sent with the request and the reply, so
/// citations of fetched pages can be matched to their URLs.
pub fn sources(context: &[Value], content: &[Value]) -> Option<String> {
    let pages = web_fetch::pages(context);
    let mut urls = Vec::new();
    let mut list = String::new();
    let citations = content
        .iter()
        .filter(|block| block["type"] == "text")
        .flat_map(|block| block["citations"].as_array().into_iter().flatten());
    for citation in citations {
        let page = citation["document_index"]
            .as_u64()
            .and_then(|index| pages.get(usize::try_from(index).ok()?));
        let (url, title) = match (citation["url"].as_str(), page) {
            (Some(url), _) => (url, citation["title"].as_str()),
            (None, Some(&(url, title))) => (url, title),
            (None, None) => continue,
        };
        if url.is_empty() || urls.contains(&url) {
            continue;
        }
        urls.push(url);
        let title = title.filter(|title| !title.is_empty()).unwrap_or(url);
        let _ = writeln!(list, "- {title}: {url}");
    }
    (!list.is_empty()).then(|| format!("\n\nSources:\n{list}"))
}
