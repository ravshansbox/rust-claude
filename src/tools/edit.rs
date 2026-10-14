use super::{argument, plural, regular_file, replace_file, tool};
use serde_json::{Value, json};

/// Larger files are refused rather than loaded whole into memory.
const MAX_SIZE: u64 = 10 * 1024 * 1024;

pub(super) fn definition() -> Value {
    tool(
        "edit",
        "Replace text in a UTF-8 file. old_text must match exactly once, unless replace_all is true",
        &json!({
            "path": { "type": "string" },
            "old_text": { "type": "string" },
            "new_text": { "type": "string" },
            "replace_all": { "type": "boolean", "description": "Replace every match of old_text. Defaults to false" }
        }),
        &["path", "old_text", "new_text"],
    )
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let path = argument(input, "path")?;
    let old_text = argument(input, "old_text")?;
    if old_text.is_empty() {
        return Err("old_text must not be empty".into());
    }
    let replace_all = match &input["replace_all"] {
        Value::Null => false,
        value => value.as_bool().ok_or("replace_all must be true or false")?,
    };
    let metadata = regular_file(path).await?;
    if metadata.len() > MAX_SIZE {
        return Err(format!("{path} is larger than 10 MiB, too large to edit"));
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("failed to read {path}: {error}"))?;
    let mut old_text = old_text.to_string();
    let mut new_text = argument(input, "new_text")?.to_string();
    let crlf = uses_crlf(&content);
    if crlf {
        new_text = to_crlf(&new_text);
        if old_text.contains('\n') && !old_text.contains('\r') {
            let converted = to_crlf(&old_text);
            if content.contains(&converted) {
                old_text = converted;
            }
        }
    }
    match count_matches(&content, &old_text) {
        0 => return Err(format!("old_text not found in {path}")),
        1 => {}
        _ if replace_all => {}
        count => return Err(format!("old_text matches {count} times in {path}")),
    }
    // A leading line break matched as the `\n` of a `\r\n` pair takes its `\r`
    // along, so the CRLF replacement does not leave a stray `\r` behind.
    let absorb_cr = crlf && old_text.starts_with('\n');
    let limit = if replace_all { usize::MAX } else { 1 };
    let (edited, count) = replace(&content, &old_text, &new_text, limit, absorb_cr);
    let message = match replace_all {
        false => format!("edited {path}"),
        true => format!("edited {path} ({})", plural(count, "replacement")),
    };
    replace_file(path, edited, Some(content))
        .await
        .map(|()| message)
        .map_err(|error| format!("failed to write {path}: {error}"))
}

/// A file uses Windows line endings when its first line ends with CRLF.
fn uses_crlf(content: &str) -> bool {
    content
        .find('\n')
        .is_some_and(|index| content[..index].ends_with('\r'))
}

fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Replaces up to `limit` non-overlapping matches, returning the new text and
/// the number of replacements. With `absorb_cr`, a `\r` right before a match
/// is replaced too.
fn replace(
    content: &str,
    old_text: &str,
    new_text: &str,
    limit: usize,
    absorb_cr: bool,
) -> (String, usize) {
    let mut result = String::with_capacity(content.len());
    let mut end = 0;
    let mut count = 0;
    for (index, matched) in content.match_indices(old_text).take(limit) {
        let start = if absorb_cr && content[end..index].ends_with('\r') {
            index - 1
        } else {
            index
        };
        result.push_str(&content[end..start]);
        result.push_str(new_text);
        end = index + matched.len();
        count += 1;
    }
    result.push_str(&content[end..]);
    (result, count)
}

fn count_matches(content: &str, pattern: &str) -> usize {
    let step = pattern.chars().next().map_or(1, char::len_utf8);
    let mut count = 0;
    let mut start = 0;
    while let Some(index) = content[start..].find(pattern) {
        count += 1;
        start += index + step;
    }
    count
}

#[cfg(test)]
mod tests;
