use super::{MAX_OUTPUT, argument, truncate};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
};

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
    let file = PathBuf::from(path);
    let read = tokio::task::spawn_blocking(move || {
        read_lines(BufReader::new(File::open(file)?), offset, limit)
    })
    .await
    .map_err(std::io::Error::other)
    .flatten()
    .map_err(|error| format!("failed to read {path}: {error}"))?;
    read.map_err(|line_count| {
        format!("offset {offset} is past the end of {path}, which has {line_count} lines")
    })
}

/// Reads `limit` lines from line `offset`, stopping once the output is full,
/// so only the part of the file that is shown gets loaded. Returns the number
/// of lines in the file when `offset` is past its end.
fn read_lines(
    mut reader: impl BufRead,
    offset: usize,
    limit: usize,
) -> std::io::Result<Result<String, usize>> {
    let mut skipped = 0;
    while skipped < offset - 1 && reader.skip_until(b'\n')? > 0 {
        skipped += 1;
    }
    let mut text = String::new();
    let mut line = Vec::new();
    for index in (offset - 1..).take(limit) {
        line.clear();
        // A few bytes more than fit, so a cut line still ends in whole
        // characters past the cut.
        let room = MAX_OUTPUT - text.len();
        (&mut reader)
            .take(room as u64 + 4)
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            if index == offset - 1 && offset > 1 {
                return Ok(Err(skipped));
            }
            break;
        }
        if line.len() > room {
            let next_line = if text.is_empty() {
                text = truncate(utf8_prefix(&line)?.to_string());
                index + 2
            } else {
                text.push_str("… output truncated");
                index + 1
            };
            text.push_str(&format!(", continue with offset {next_line}"));
            return Ok(Ok(text));
        }
        text.push_str(std::str::from_utf8(&line).map_err(|_| not_utf8())?);
    }
    Ok(Ok(text))
}

/// The text in `bytes`, which may end partway through a character.
fn utf8_prefix(bytes: &[u8]) -> std::io::Result<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if error.error_len().is_none() => utf8_prefix(&bytes[..error.valid_up_to()]),
        Err(_) => Err(not_utf8()),
    }
}

fn not_utf8() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "stream did not contain valid UTF-8",
    )
}

#[cfg(test)]
mod tests {
    use super::read_lines as read;
    use crate::tools::{MAX_OUTPUT, call, test_support::TemporaryFile};
    use serde_json::json;
    use std::time::Duration;

    fn read_lines(content: &str, offset: usize, limit: usize) -> Result<String, usize> {
        read(content.as_bytes(), offset, limit).unwrap()
    }

    #[test]
    fn reads_requested_lines() {
        let content = "one\ntwo\nthree\nfour\n";
        assert_eq!(read_lines(content, 1, usize::MAX), Ok(content.into()));
        assert_eq!(read_lines(content, 2, 2), Ok("two\nthree\n".into()));
        assert_eq!(read_lines(content, 10, usize::MAX), Err(4));
    }

    #[test]
    fn cuts_long_output_at_whole_lines() {
        let line = format!("{}\n", "a".repeat(99));
        let content = line.repeat(MAX_OUTPUT / 100 + 10);
        let text = read_lines(&content, 1, usize::MAX).unwrap();
        let next_line = MAX_OUTPUT / 100 + 1;
        assert!(text.starts_with(&line.repeat(MAX_OUTPUT / 100)));
        assert!(text.ends_with(&format!(
            "… output truncated, continue with offset {next_line}"
        )));
    }

    #[test]
    fn cuts_single_long_line() {
        let content = format!("{}\nnext\n", "a".repeat(MAX_OUTPUT + 1));
        let text = read_lines(&content, 1, usize::MAX).unwrap();
        assert!(text.ends_with("… output truncated, continue with offset 2"));
        assert_eq!(read_lines(&content, 2, usize::MAX), Ok("next\n".into()));
    }

    #[test]
    fn cuts_long_line_inside_multibyte_character() {
        let content = format!("a{}", "é".repeat(MAX_OUTPUT));
        let text = read_lines(&content, 1, usize::MAX).unwrap();
        assert!(text.starts_with(&format!("a{}", "é".repeat(MAX_OUTPUT / 2 - 1))));
        assert!(text.ends_with("… output truncated, continue with offset 2"));
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
    async fn reads_first_lines_without_waiting_for_the_rest() {
        let fifo = TemporaryFile::fifo("read-fifo");
        let path = fifo.path().to_path_buf();
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            file.write_all(b"one\ntwo\n").unwrap();
            let _ = finished.recv();
        });
        let input = json!({ "path": fifo.path(), "limit": 1 });
        let read = tokio::time::timeout(Duration::from_secs(5), call("read", &input)).await;
        done.send(()).unwrap();
        writer.join().unwrap();
        assert_eq!(read, Ok(Ok("one\n".into())));
    }
}
