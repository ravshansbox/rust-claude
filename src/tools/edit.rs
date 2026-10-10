use super::{argument, replace_file};
use serde_json::{Value, json};

/// Larger files are refused rather than loaded whole into memory.
const MAX_SIZE: u64 = 10 * 1024 * 1024;

pub(super) fn definition() -> Value {
    json!({
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
    })
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let path = argument(input, "path")?;
    let old_text = argument(input, "old_text")?;
    if old_text.is_empty() {
        return Err("old_text must not be empty".into());
    }
    let replace_all = match &input["replace_all"] {
        Value::Null => false,
        value => value.as_bool().ok_or("replace_all must be true or false")?,
    };
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("failed to read {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    if metadata.len() > MAX_SIZE {
        return Err(format!("{path} is larger than 10 MiB, too large to edit"));
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("failed to read {path}: {error}"))?;
    let mut old_text = old_text.to_string();
    let mut new_text = argument(input, "new_text")?.to_string();
    if uses_crlf(&content) {
        new_text = to_crlf(&new_text);
        if count_matches(&content, &old_text) == 0 && !old_text.contains('\r') {
            old_text = to_crlf(&old_text);
        }
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
    replace_file(path, content)
        .await
        .map(|_| message)
        .map_err(|error| format!("failed to write {path}: {error}"))
}

/// A file uses Windows line endings when its first line ends with CRLF.
fn uses_crlf(content: &str) -> bool {
    content
        .find('\n')
        .is_some_and(|index| content[..index].ends_with('\r'))
}

fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
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

#[cfg(test)]
mod tests {
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
        let input =
            json!({ "path": file.path(), "old_text": "one\ntwo", "new_text": "one\nnew\ntwo" });
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

    #[cfg(unix)]
    #[tokio::test]
    async fn keeps_permissions_of_edited_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TemporaryDir::new("edit-mode");
        let path = directory.path().join("script.sh");
        std::fs::write(&path, "echo old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        let input = json!({ "path": path, "old_text": "old", "new_text": "new" });
        assert_eq!(
            call("edit", &input).await,
            Ok(format!("edited {}", path.display()))
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "echo new\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750);
        assert_eq!(directory.entries(), ["script.sh"]);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn edits_file_in_read_only_folder() {
        let directory = TemporaryDir::new("edit-read-only");
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "old").unwrap();
        // The file stays writable, but no file can be added next to it.
        directory.set_mode(0o500);
        let input = json!({ "path": path, "old_text": "old", "new_text": "new" });
        let result = call("edit", &input).await;
        let content = std::fs::read_to_string(&path).unwrap();
        let entries = directory.entries();
        directory.set_mode(0o700);
        assert_eq!(result, Ok(format!("edited {}", path.display())));
        assert_eq!(content, "new");
        assert_eq!(entries, ["file.txt"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_read_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TemporaryDir::new("edit-read-only-file");
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let input = json!({ "path": path, "old_text": "old", "new_text": "new" });
        let result = call("edit", &input).await;
        assert!(
            result.as_ref().is_err_and(
                |error| error.starts_with(&format!("failed to write {}: ", path.display()))
            ),
            "{result:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(directory.entries(), ["file.txt"]);
    }
}
