use serde_json::{Value, json};

mod bash;
mod display;
mod read;
#[cfg(test)]
mod test_support;
mod write;

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

pub fn definitions() -> Value {
    json!([
        bash::definition(),
        read::definition(),
        write::definition(),
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
        "read" => read::run(input).await,
        "write" => write::run(input).await,
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
    use super::{MAX_OUTPUT, call};
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
}
