use super::{MAX_OUTPUT, argument, truncate};
use serde_json::{Value, json};

pub(super) fn definition() -> Value {
    json!({
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
    })
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::read_lines;
    use crate::tools::{MAX_OUTPUT, call, test_support::TemporaryFile};
    use serde_json::json;

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
    fn cuts_single_long_line() {
        let content = format!("{}\nnext\n", "a".repeat(MAX_OUTPUT + 1));
        let text = read_lines(&content, 1, usize::MAX);
        assert!(text.ends_with("… output truncated, continue with offset 2"));
        assert_eq!(read_lines(&content, 2, usize::MAX), "next\n");
    }

    #[tokio::test]
    async fn rejects_offset_past_end_of_file() {
        let file = TemporaryFile::new("offset", "one\ntwo\n");
        let past_end = call("read", &json!({ "path": file.path(), "offset": 3 })).await;
        let last_line = call("read", &json!({ "path": file.path(), "offset": 2 })).await;
        assert!(past_end.unwrap_err().contains("which has 2 lines"));
        assert_eq!(last_line, Ok("two\n".into()));
    }
}
