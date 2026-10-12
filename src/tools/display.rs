use serde_json::Value;
use std::borrow::Cow;
use std::fmt::Write;

pub fn summary(name: &str, input: &Value) -> String {
    let key = match name {
        "read" | "write" | "edit" => "path",
        super::web_search::NAME => "query",
        "bash" if super::background::runs_in_background(name, input) => {
            return "in the background".into();
        }
        "bash" => return String::new(),
        super::background::STOP => {
            return format!("background task {}", input["id"]);
        }
        super::agent::NAME => return super::agent::summary(input),
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
        "write" => write_preview(input["content"].as_str()?),
        "bash" => code_preview(input["command"].as_str()?),
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

fn code_preview(code: &str) -> Option<String> {
    if code.is_empty() {
        return None;
    }
    let unit = indent_unit(code.lines());
    let mut preview = String::with_capacity(code.len());
    for (index, line) in code.lines().enumerate() {
        if index > 0 {
            preview.push('\n');
        }
        push_compact_indent(&mut preview, line, unit);
    }
    Some(preview)
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

fn write_preview(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let unit = indent_unit(lines.iter().copied());
    let mut preview = String::with_capacity(content.len());
    for (index, line) in lines.iter().take(WRITE_PREVIEW_LINES).enumerate() {
        if index > 0 {
            preview.push('\n');
        }
        preview.push(' ');
        push_compact_indent(&mut preview, line, unit);
    }
    let remaining = lines.len().saturating_sub(WRITE_PREVIEW_LINES);
    if remaining > 0 {
        let noun = if remaining == 1 { "line" } else { "lines" };
        let _ = write!(preview, "\n… {remaining} more {noun}");
    }
    Some(preview)
}

#[cfg(test)]
mod tests {
    use super::{diff, note, summary};
    use serde_json::json;

    #[test]
    fn summarizes_reads_with_huge_limits() {
        let input = json!({ "path": "a.txt", "offset": 5, "limit": u64::MAX });
        assert_eq!(summary("read", &input), format!("a.txt:5-{}", u64::MAX));
    }

    #[test]
    fn previews_first_lines_of_write() {
        let content: String = (1..=12).map(|number| format!("line {number}\n")).collect();
        let preview = diff("write", &json!({ "path": "a.txt", "content": content })).unwrap();
        let lines: Vec<&str> = preview.lines().collect();
        assert_eq!(lines.len(), 11);
        assert_eq!(lines[0], " line 1");
        assert_eq!(lines[9], " line 10");
        assert_eq!(lines[10], "… 2 more lines");
    }

    #[test]
    fn shows_read_range_in_summary() {
        let cases = [
            (json!({ "path": "a.rs" }), "a.rs"),
            (
                json!({ "path": "a.rs", "offset": 325, "limit": 30 }),
                "a.rs:325-354",
            ),
            (json!({ "path": "a.rs", "offset": 325 }), "a.rs:325-"),
            (json!({ "path": "a.rs", "limit": 30 }), "a.rs:1-30"),
        ];
        for (input, expected) in cases {
            assert_eq!(summary("read", &input), expected);
        }
    }

    #[test]
    fn notes_replacement_count() {
        assert_eq!(
            note("edit", "edited a (b).rs (3 replacements)"),
            Some("3 replacements".into())
        );
        assert_eq!(
            note("edit", "edited a.rs (1 replacement)"),
            Some("1 replacement".into())
        );
        assert_eq!(note("edit", "edited a (b).rs"), None);
        assert_eq!(note("bash", "x (3 replacements)"), None);
    }

    #[test]
    fn diffs_edit_input() {
        let input = json!({ "path": "a", "old_text": "a\nb\n", "new_text": "a\nc\n" });
        assert_eq!(diff("edit", &input), Some(" a\n-b\n+c".into()));
        assert_eq!(diff("write", &input), None);
    }

    #[test]
    fn keeps_last_line_unchanged_when_lines_are_appended_without_newline() {
        let input = json!({ "path": "a", "old_text": "foo()", "new_text": "foo()\nbar()" });
        assert_eq!(diff("edit", &input), Some(" foo()\n+bar()".into()));
    }

    #[test]
    fn shows_bash_command_as_code() {
        let input = json!({ "command": "for x in a b; do\n    echo $x\ndone" });
        assert_eq!(summary("bash", &input), "");
        assert_eq!(
            diff("bash", &input),
            Some("for x in a b; do\n echo $x\ndone".into())
        );
    }

    #[test]
    fn keeps_one_space_per_indentation_level_in_previews() {
        let edit = json!({ "path": "a", "old_text": "a\n    b\n", "new_text": "a\n        c\n" });
        assert_eq!(diff("edit", &edit), Some(" a\n- b\n+  c".into()));
        let tabs = json!({ "path": "a", "old_text": "a\n\tb\n", "new_text": "a\n\t\tc\n" });
        assert_eq!(diff("edit", &tabs), Some(" a\n- b\n+  c".into()));
        let write = json!({ "path": "a", "content": "fn main() {\n  if x {\n    body\n  }\n}\n" });
        assert_eq!(
            diff("write", &write),
            Some(" fn main() {\n  if x {\n   body\n  }\n }".into())
        );
    }
}
