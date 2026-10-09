use serde_json::{Value, json};

mod bash;
mod display;
#[cfg(test)]
mod test_support;

pub use bash::ProcessGroup;
pub use display::{ReadGroup, diff, note, summary};

const MAX_OUTPUT: usize = 20_000;

pub fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… output truncated");
    }
    text
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

fn read_lines(content: &str, offset: usize, limit: usize) -> String {
    let mut text = String::new();
    let lines = content
        .split_inclusive('\n')
        .enumerate()
        .skip(offset - 1)
        .take(limit);
    for (index, line) in lines {
        if text.len() + line.len() > MAX_OUTPUT {
            let next_line = if text.is_empty() {
                text = truncate(line.to_string());
                index + 2
            } else {
                text.push_str("… output truncated");
                index + 1
            };
            text.push_str(&format!(", continue with offset {next_line}"));
            return text;
        }
        text.push_str(line);
    }
    text
}

pub fn definitions() -> Value {
    json!([
        bash::definition(),
        {
            "name": "read",
            "description": "Read a UTF-8 file from the current project. Long output is cut at whole lines; use offset and limit to read the rest",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "description": "Line number to start from, counting from 1" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "write",
            "description": "Create or replace a UTF-8 file in the current project",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "edit",
            "description": "Replace text in a UTF-8 file. old_text must match exactly once, unless replace_all is true",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" },
                    "replace_all": { "type": "boolean", "description": "Replace every match of old_text. Defaults to false" }
                },
                "required": ["path", "old_text", "new_text"]
            }
        }
    ])
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
}

pub async fn call(name: &str, input: &Value) -> Result<String, String> {
    match name {
        "bash" => bash::run(input).await,
        "read" => {
            let path = argument(input, "path")?;
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            let offset = match &input["offset"] {
                Value::Null => 1,
                value => match value.as_u64() {
                    Some(offset) => offset.max(1) as usize,
                    None => return Err("offset must be at least 1".into()),
                },
            };
            let limit = match &input["limit"] {
                Value::Null => usize::MAX,
                value => match value.as_u64() {
                    Some(limit) if limit > 0 => limit as usize,
                    _ => return Err("limit must be at least 1".into()),
                },
            };
            let line_count = content.split_inclusive('\n').count();
            if offset > line_count.max(1) {
                return Err(format!(
                    "offset {offset} is past the end of {path}, which has {line_count} lines"
                ));
            }
            Ok(read_lines(&content, offset, limit))
        }
        "write" => {
            let path = argument(input, "path")?;
            let content = argument(input, "content")?;
            if let Some(parent) = std::path::Path::new(path).parent()
                && !parent.as_os_str().is_empty()
            {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| format!("failed to write {path}: {error}"))?;
            }
            tokio::fs::write(path, content)
                .await
                .map(|_| format!("wrote {path}"))
                .map_err(|error| format!("failed to write {path}: {error}"))
        }
        "edit" => {
            let path = argument(input, "path")?;
            let old_text = argument(input, "old_text")?;
            if old_text.is_empty() {
                return Err("old_text must not be empty".into());
            }
            let replace_all = match &input["replace_all"] {
                Value::Null => false,
                value => value.as_bool().ok_or("replace_all must be true or false")?,
            };
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            let mut old_text = old_text.to_string();
            let mut new_text = argument(input, "new_text")?.to_string();
            if count_matches(&content, &old_text) == 0
                && content.contains("\r\n")
                && !old_text.contains('\r')
            {
                old_text = old_text.replace('\n', "\r\n");
                new_text = new_text.replace("\r\n", "\n").replace('\n', "\r\n");
            }
            match count_matches(&content, &old_text) {
                0 => return Err(format!("old_text not found in {path}")),
                1 => {}
                _ if replace_all => {}
                count => return Err(format!("old_text matches {count} times in {path}")),
            }
            let (content, message) = if replace_all {
                let count = content.matches(old_text.as_str()).count();
                let noun = if count == 1 {
                    "replacement"
                } else {
                    "replacements"
                };
                (
                    content.replace(&old_text, &new_text),
                    format!("edited {path} ({count} {noun})"),
                )
            } else {
                (
                    content.replacen(&old_text, &new_text, 1),
                    format!("edited {path}"),
                )
            };
            tokio::fs::write(path, content)
                .await
                .map(|_| message)
                .map_err(|error| format!("failed to write {path}: {error}"))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TemporaryFile;
    use super::{MAX_OUTPUT, call, read_lines};
    use serde_json::json;

    #[tokio::test]
    async fn rejects_invalid_numbers() {
        let cases = [
            (
                "bash",
                json!({ "command": "echo hi", "timeout": 0 }),
                "timeout must be at least 1",
            ),
            (
                "bash",
                json!({ "command": "echo hi", "timeout": -1 }),
                "timeout must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "limit": 0 }),
                "limit must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "limit": -1 }),
                "limit must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "offset": -5 }),
                "offset must be at least 1",
            ),
        ];
        for (name, input, error) in cases {
            assert_eq!(call(name, &input).await, Err(error.into()), "{input}");
        }
    }

    #[tokio::test]
    async fn writes_file_in_new_directory() {
        let directory = std::env::temp_dir().join(format!("rust-claude-{}", std::process::id()));
        let path = directory.join("nested").join("file.txt");
        let input = json!({ "path": path.to_str().unwrap(), "content": "hello" });
        let result = call("write", &input).await;
        let content = std::fs::read_to_string(&path);
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(result.is_ok());
        assert_eq!(content.unwrap(), "hello");
    }

    #[tokio::test]
    async fn write_without_content_creates_no_directory() {
        let directory =
            std::env::temp_dir().join(format!("rust-claude-empty-{}", std::process::id()));
        let path = directory.join("file.txt");
        let input = json!({ "path": path.to_str().unwrap() });
        let result = call("write", &input).await;
        let created = directory.exists();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(result, Err("missing argument: content".into()));
        assert!(!created);
    }

    #[test]
    fn reads_requested_lines() {
        let content = "one\ntwo\nthree\nfour\n";
        assert_eq!(read_lines(content, 1, usize::MAX), content);
        assert_eq!(read_lines(content, 2, 2), "two\nthree\n");
        assert_eq!(read_lines(content, 10, usize::MAX), "");
    }

    #[test]
    fn cuts_long_output_at_whole_lines() {
        let line = format!("{}\n", "a".repeat(99));
        let content = line.repeat(MAX_OUTPUT / 100 + 10);
        let text = read_lines(&content, 1, usize::MAX);
        let next_line = MAX_OUTPUT / 100 + 1;
        assert!(text.starts_with(&line.repeat(MAX_OUTPUT / 100)));
        assert!(text.ends_with(&format!(
            "… output truncated, continue with offset {next_line}"
        )));
    }

    #[test]
    fn truncates_inside_multibyte_character() {
        let text = super::truncate(format!("a{}", "é".repeat(MAX_OUTPUT)));
        assert!(text.ends_with("\n… output truncated"));
        assert_eq!(text.len(), MAX_OUTPUT - 1 + "\n… output truncated".len());
    }

    #[tokio::test]
    async fn rejects_empty_old_text() {
        let file = TemporaryFile::new("edit", "");
        let input = json!({ "path": file.path(), "old_text": "", "new_text": "added" });
        assert_eq!(
            call("edit", &input).await,
            Err("old_text must not be empty".into())
        );
        assert_eq!(file.content(), "");
    }

    #[tokio::test]
    async fn rejects_offset_past_end_of_file() {
        let file = TemporaryFile::new("offset", "one\ntwo\n");
        let past_end = call("read", &json!({ "path": file.path(), "offset": 3 })).await;
        let last_line = call("read", &json!({ "path": file.path(), "offset": 2 })).await;
        assert!(past_end.unwrap_err().contains("which has 2 lines"));
        assert_eq!(last_line, Ok("two\n".into()));
    }

    #[tokio::test]
    async fn rejects_overlapping_old_text() {
        let file = TemporaryFile::new("overlap", "aaa");
        let input = json!({ "path": file.path(), "old_text": "aa", "new_text": "b" });
        assert!(
            call("edit", &input)
                .await
                .unwrap_err()
                .contains("matches 2 times")
        );
        assert_eq!(file.content(), "aaa");
    }

    #[tokio::test]
    async fn replaces_all_matches() {
        let file = TemporaryFile::new("replace-all", "a b a");
        let input =
            json!({ "path": file.path(), "old_text": "a", "new_text": "c", "replace_all": true });
        assert_eq!(
            call("edit", &input).await,
            Ok(format!("edited {} (2 replacements)", file.path().display()))
        );
        assert_eq!(file.content(), "c b c");
        let input =
            json!({ "path": file.path(), "old_text": "a", "new_text": "c", "replace_all": true });
        assert!(
            call("edit", &input)
                .await
                .unwrap_err()
                .contains("not found")
        );
    }

    #[tokio::test]
    async fn counts_replacements_made() {
        let file = TemporaryFile::new("replace-all-overlap", "aaa");
        let input =
            json!({ "path": file.path(), "old_text": "aa", "new_text": "b", "replace_all": true });
        assert_eq!(
            call("edit", &input).await,
            Ok(format!("edited {} (1 replacement)", file.path().display()))
        );
        assert_eq!(file.content(), "ba");
    }

    #[tokio::test]
    async fn rejects_invalid_replace_all() {
        let file = TemporaryFile::new("replace-all-invalid", "a b a");
        let input =
            json!({ "path": file.path(), "old_text": "a", "new_text": "c", "replace_all": "yes" });
        assert_eq!(
            call("edit", &input).await,
            Err("replace_all must be true or false".into())
        );
        assert_eq!(file.content(), "a b a");
    }

    #[tokio::test]
    async fn edits_file_with_crlf_line_endings() {
        let file = TemporaryFile::new("crlf", "one\r\ntwo\r\nthree\r\n");
        let input =
            json!({ "path": file.path(), "old_text": "one\ntwo", "new_text": "one\nnew\ntwo" });
        assert_eq!(
            call("edit", &input).await,
            Ok(format!("edited {}", file.path().display()))
        );
        assert_eq!(file.content(), "one\r\nnew\r\ntwo\r\nthree\r\n");
    }

    #[test]
    fn cuts_single_long_line() {
        let content = format!("{}\nnext\n", "a".repeat(MAX_OUTPUT + 1));
        let text = read_lines(&content, 1, usize::MAX);
        assert!(text.ends_with("… output truncated, continue with offset 2"));
        assert_eq!(read_lines(&content, 2, usize::MAX), "next\n");
    }
}
