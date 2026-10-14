use super::{agent, background, plural, web_search};
use serde_json::Value;
use std::borrow::Cow;
use std::fmt::Write;

pub fn summary(name: &str, input: &Value) -> String {
    let key = match name {
        "read" | "write" | "edit" => "path",
        web_search::NAME => "query",
        "bash" if background::runs_in_background(name, input) => {
            return "in the background".into();
        }
        "bash" => return String::new(),
        background::STOP => {
            return format!("background task {}", input["id"]);
        }
        agent::NAME => return agent::summary(input),
        _ => return input.to_string(),
    };
    let value = input[key].as_str().unwrap_or_default().to_string();
    if name != "read" {
        return value;
    }
    let offset = input["offset"].as_u64();
    let limit = input["limit"].as_u64();
    let start = offset.unwrap_or(1).max(1);
    match (offset, limit) {
        (None, None) => value,
        (_, Some(limit)) => format!("{value}:{start}-{}", start.saturating_add(limit.max(1) - 1)),
        (Some(_), None) => format!("{value}:{start}-"),
    }
}

pub fn note(name: &str, result: &str) -> Option<String> {
    if name != "edit" {
        return None;
    }
    let (_, count) = result.strip_suffix(')')?.rsplit_once(" (")?;
    (count.ends_with(" replacement") || count.ends_with(" replacements")).then(|| count.into())
}

#[derive(Default)]
pub struct ReadGroup {
    paths: Vec<(String, usize)>,
}

impl ReadGroup {
    pub fn add(&mut self, path: String) {
        match self
            .paths
            .iter_mut()
            .find(|(existing, _)| *existing == path)
        {
            Some((_, count)) => *count += 1,
            None => self.paths.push((path, 1)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn summary(&self) -> String {
        self.paths
            .iter()
            .map(|(path, count)| match count {
                1 => path.clone(),
                _ => format!("{path} ({count})"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

const WRITE_PREVIEW_LINES: usize = 10;

pub fn diff(name: &str, input: &Value) -> Option<String> {
    match name {
        "write" => preview(input["content"].as_str()?, " ", WRITE_PREVIEW_LINES),
        "bash" => preview(input["command"].as_str()?, "", usize::MAX),
        "edit" => Some(edit_preview(
            input["old_text"].as_str()?,
            input["new_text"].as_str()?,
        )),
        _ => None,
    }
}

fn edit_preview(old_text: &str, new_text: &str) -> String {
    let old_text = terminated(old_text);
    let new_text = terminated(new_text);
    let unit = indent_unit(old_text.lines().chain(new_text.lines()));
    let diff = similar::TextDiff::from_lines(&*old_text, &*new_text);
    let mut preview = String::with_capacity(old_text.len() + new_text.len());
    for (index, change) in diff.iter_all_changes().enumerate() {
        if index > 0 {
            preview.push('\n');
        }
        preview.push(match change.tag() {
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Insert => '+',
            similar::ChangeTag::Equal => ' ',
        });
        push_compact_indent(&mut preview, change.value().trim_end_matches('\n'), unit);
    }
    preview
}

// "x" and "x\n" count as different lines, so end both texts the same way
// to keep an unchanged last line from showing as removed and re-added.
fn terminated(text: &str) -> Cow<'_, str> {
    if text.is_empty() || text.ends_with('\n') {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(format!("{text}\n"))
    }
}

fn leading_spaces(line: &str) -> usize {
    line.trim_start_matches('\t')
        .chars()
        .take_while(|character| *character == ' ')
        .count()
}

fn indent_unit<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    lines
        .filter(|line| !line.trim().is_empty())
        .map(leading_spaces)
        .filter(|spaces| *spaces > 0)
        .min()
        .unwrap_or(1)
}

fn push_compact_indent(preview: &mut String, line: &str, unit: usize) {
    let content = line.trim_start();
    if content.is_empty() {
        return;
    }
    let tabs = line.len() - line.trim_start_matches('\t').len();
    let levels = tabs + leading_spaces(line).div_ceil(unit);
    preview.extend(std::iter::repeat_n(' ', levels));
    preview.push_str(content);
}

fn preview(content: &str, prefix: &str, max_lines: usize) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let unit = indent_unit(lines.iter().copied());
    let mut preview = String::with_capacity(content.len());
    for (index, line) in lines.iter().take(max_lines).enumerate() {
        if index > 0 {
            preview.push('\n');
        }
        preview.push_str(prefix);
        push_compact_indent(&mut preview, line, unit);
    }
    let remaining = lines.len().saturating_sub(max_lines);
    if remaining > 0 {
        let _ = write!(preview, "\n… {}", plural(remaining, "more line"));
    }
    Some(preview)
}

#[cfg(test)]
mod tests;
