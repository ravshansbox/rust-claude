use super::read_lines as read;
use crate::tools::{MAX_OUTPUT, call, test_support::TemporaryFile};
use serde_json::json;
use std::time::Duration;

fn read_lines(content: &str, offset: usize, limit: usize) -> Result<String, usize> {
    read(content.as_bytes(), offset, limit).unwrap()
}

#[tokio::test]
async fn reads_requested_lines() {
    let file = TemporaryFile::new("lines", "one\ntwo\nthree\nfour");
    let read = async |offset: u64, limit: Option<u64>| {
        let mut input = json!({ "path": file.path(), "offset": offset });
        if let Some(limit) = limit {
            input["limit"] = json!(limit);
        }
        call("read", &input).await
    };
    assert_eq!(read(0, None).await, Ok("one\ntwo\nthree\nfour".into()));
    assert_eq!(read(2, Some(2)).await, Ok("two\nthree\n".into()));
    assert_eq!(read(4, Some(5)).await, Ok("four".into()));
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
async fn reads_text_that_starts_like_an_image_header_as_text() {
    let file = TemporaryFile::new("image-like", "BM is a bitmap header\nP1 too\n");
    let read = call("read", &json!({ "path": file.path() })).await;
    assert_eq!(read, Ok("BM is a bitmap header\nP1 too\n".into()));
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
async fn names_a_single_line_in_the_singular_when_offset_is_past_the_end() {
    let file = TemporaryFile::new("offset-one-line", "one\n");
    let past_end = call("read", &json!({ "path": file.path(), "offset": 3 })).await;
    assert!(past_end.unwrap_err().ends_with("which has 1 line"));
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
