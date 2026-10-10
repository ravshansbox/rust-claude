use serde_json::Value;

pub fn summary(name: &str, input: &Value) -> String {
    let key = match name {
        "bash" => "command",
        "read" | "write" | "edit" => "path",
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
    if name == "write" {
        return write_preview(input["content"].as_str()?);
    }
    if name != "edit" {
        return None;
    }
    // "x" and "x\n" count as different lines, so end both texts the same way
    // to keep an unchanged last line from showing as removed and re-added.
    let terminated = |text: &str| {
        if text.is_empty() || text.ends_with('\n') {
            text.to_string()
        } else {
            format!("{text}\n")
        }
    };
    let old_text = terminated(input["old_text"].as_str()?);
    let new_text = terminated(input["new_text"].as_str()?);
    let diff = similar::TextDiff::from_lines(&old_text, &new_text);
    let lines: Vec<String> = diff
        .iter_all_changes()
        .map(|change| {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => '-',
                similar::ChangeTag::Insert => '+',
                similar::ChangeTag::Equal => ' ',
            };
            format!(
                "{sign}{}",
                change.value().trim_end_matches('\n').trim_start()
            )
        })
        .collect();
    Some(lines.join("\n"))
}

fn write_preview(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let mut preview: Vec<String> = lines
        .iter()
        .take(WRITE_PREVIEW_LINES)
        .map(|line| format!(" {}", line.trim_start()))
        .collect();
    let remaining = lines.len().saturating_sub(WRITE_PREVIEW_LINES);
    if remaining > 0 {
        let noun = if remaining == 1 { "line" } else { "lines" };
        preview.push(format!("… {remaining} more {noun}"));
    }
    Some(preview.join("\n"))
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
    fn trims_leading_spaces_in_previews() {
        let edit = json!({ "path": "a", "old_text": "    a\n\tb\n", "new_text": "    a\n  c\n" });
        assert_eq!(diff("edit", &edit), Some(" a\n-b\n+c".into()));
        let write = json!({ "path": "a", "content": "fn main() {\n    body\n}\n" });
        assert_eq!(
            diff("write", &write),
            Some(" fn main() {\n body\n }".into())
        );
    }
}
