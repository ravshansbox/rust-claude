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
        (
            "bash",
            json!({ "command": "echo hi", "timeout": "30" }),
            "timeout must be a whole number",
        ),
        (
            "read",
            json!({ "path": "Cargo.toml", "limit": 1.5 }),
            "limit must be a whole number",
        ),
    ];
    for (name, input, error) in cases {
        assert_eq!(call(name, &input).await, Err(error.into()), "{input}");
    }
}

#[tokio::test]
async fn refuses_to_replace_a_file_that_changed_since_it_was_read() {
    let file = super::test_support::TemporaryFile::new("changed", "changed by someone else");
    let path = file.path().to_str().unwrap();
    let result = super::replace_file(path, "edited".into(), Some("as read".into())).await;
    assert_eq!(
        result.map_err(|error| error.to_string()),
        Err("it changed while being edited".into())
    );
    assert_eq!(file.content(), "changed by someone else");
}

#[test]
fn truncates_inside_multibyte_character() {
    let text = super::truncate(format!("a{}", "é".repeat(MAX_OUTPUT)));
    assert!(text.ends_with("\n… output truncated"));
    assert_eq!(text.len(), MAX_OUTPUT - 1 + "\n… output truncated".len());
}
