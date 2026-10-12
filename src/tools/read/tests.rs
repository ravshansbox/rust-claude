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
async fn refuses_files_that_are_not_utf8_text() {
    let file = TemporaryFile::new("binary", "");
    for content in [
        b"text\n\xff\xfe binary\n".to_vec(),
        [b"\xff".repeat(MAX_OUTPUT * 2), b"\n".to_vec()].concat(),
    ] {
        std::fs::write(file.path(), content).unwrap();
        let error = call("read", &json!({ "path": file.path() }))
            .await
            .unwrap_err();
        assert!(
            error.contains("stream did not contain valid UTF-8"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn rejects_offset_past_end_of_file() {
    let file = TemporaryFile::new("offset", "one\ntwo\n");
    let past_end = call("read", &json!({ "path": file.path(), "offset": 3 })).await;
    let last_line = call("read", &json!({ "path": file.path(), "offset": 2 })).await;
    assert!(past_end.unwrap_err().contains("which has 2 lines"));
    assert_eq!(last_line, Ok("two\n".into()));
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_pipes_and_devices() {
    let fifo = TemporaryFile::fifo("read-fifo");
    let fifo = fifo.path().to_str().unwrap();
    for path in [fifo, "/dev/zero"] {
        let input = json!({ "path": path, "offset": 2 });
        let read = tokio::time::timeout(Duration::from_secs(5), call("read", &input)).await;
        assert_eq!(read, Ok(Err(format!("{path} is not a regular file"))));
    }
}
