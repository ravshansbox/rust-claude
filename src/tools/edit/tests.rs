use crate::tools::{
    call,
    test_support::{TemporaryDir, TemporaryFile},
};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn refuses_files_that_are_not_regular() {
    let fifo = TemporaryFile::fifo("edit-fifo");
    let input = json!({ "path": fifo.path(), "old_text": "a", "new_text": "b" });
    let edit = tokio::time::timeout(Duration::from_secs(5), call("edit", &input)).await;
    assert_eq!(
        edit,
        Ok(Err(format!(
            "{} is not a regular file",
            fifo.path().display()
        )))
    );
}

#[tokio::test]
async fn refuses_files_over_10_mib() {
    let file = TemporaryFile::new("edit-large", "");
    std::fs::File::options()
        .write(true)
        .open(file.path())
        .unwrap()
        .set_len(10 * 1024 * 1024 + 1)
        .unwrap();
    let input = json!({ "path": file.path(), "old_text": "a", "new_text": "b" });
    assert_eq!(
        call("edit", &input).await,
        Err(format!(
            "{} is larger than 10 MiB, too large to edit",
            file.path().display()
        ))
    );
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
    let input = json!({ "path": file.path(), "old_text": "one\ntwo", "new_text": "one\nnew\ntwo" });
    assert_eq!(
        call("edit", &input).await,
        Ok(format!("edited {}", file.path().display()))
    );
    assert_eq!(file.content(), "one\r\nnew\r\ntwo\r\nthree\r\n");
}

#[tokio::test]
async fn keeps_crlf_when_one_line_becomes_several() {
    let file = TemporaryFile::new("crlf-one-line", "one\r\ntwo\r\n");
    let input = json!({ "path": file.path(), "old_text": "one", "new_text": "first\nsecond" });
    assert_eq!(
        call("edit", &input).await,
        Ok(format!("edited {}", file.path().display()))
    );
    assert_eq!(file.content(), "first\r\nsecond\r\ntwo\r\n");
}

#[tokio::test]
async fn keeps_crlf_when_old_text_starts_with_line_break() {
    let file = TemporaryFile::new("crlf-leading-break", "a();\r\n    gone();\r\nb();\r\n");
    let input = json!({ "path": file.path(), "old_text": "\n    gone();", "new_text": "" });
    assert_eq!(
        call("edit", &input).await,
        Ok(format!("edited {}", file.path().display()))
    );
    assert_eq!(file.content(), "a();\r\nb();\r\n");

    let file = TemporaryFile::new("crlf-leading-break-replace", "x\r\nfoo\r\nfoo");
    let input = json!({
        "path": file.path(),
        "old_text": "\nfoo",
        "new_text": "\nbar",
        "replace_all": true,
    });
    assert_eq!(
        call("edit", &input).await,
        Ok(format!("edited {} (2 replacements)", file.path().display()))
    );
    assert_eq!(file.content(), "x\r\nbar\r\nbar");
}

#[tokio::test]
async fn keeps_crlf_when_only_lf_spelling_of_old_text_matches() {
    // A mixed file: the CRLF spelling of old_text is missing, so the
    // edit falls back to the text as given.
    let file = TemporaryFile::new("crlf-mixed", "a\r\nfoo\nbar\r\n");
    let input = json!({ "path": file.path(), "old_text": "\nfoo\nbar", "new_text": "" });
    assert_eq!(
        call("edit", &input).await,
        Ok(format!("edited {}", file.path().display()))
    );
    assert_eq!(file.content(), "a\r\n");
}

#[cfg(unix)]
#[tokio::test]
async fn edits_target_of_symlink() {
    let directory = TemporaryDir::new("edit-symlink");
    let target = directory.path().join("target.txt");
    let link = directory.path().join("link.txt");
    std::fs::write(&target, "old").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let input = json!({ "path": link, "old_text": "old", "new_text": "new" });
    assert_eq!(
        call("edit", &input).await,
        Ok(format!("edited {}", link.display()))
    );
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(directory.entries(), ["link.txt", "target.txt"]);
}
