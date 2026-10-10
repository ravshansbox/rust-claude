use super::{argument, replace_file};
use serde_json::{Value, json};

pub(super) fn definition() -> Value {
    json!({
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
    })
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let path = argument(input, "path")?;
    let content = argument(input, "content")?;
    if let Some(parent) = std::path::Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("failed to write {path}: {error}"))?;
    }
    replace_file(path, content.to_string())
        .await
        .map(|_| format!("wrote {path}"))
        .map_err(|error| format!("failed to write {path}: {error}"))
}

#[cfg(test)]
mod tests {
    use crate::tools::{
        call,
        test_support::{TemporaryDir, TemporaryFile},
    };
    use serde_json::json;

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

    #[cfg(unix)]
    #[tokio::test]
    async fn replaces_target_of_symlink_and_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TemporaryDir::new("write-symlink");
        let target = directory.path().join("script.sh");
        let link = directory.path().join("link.sh");
        std::fs::write(&target, "echo old\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let input = json!({ "path": link, "content": "echo new\n" });
        assert_eq!(
            call("write", &input).await,
            Ok(format!("wrote {}", link.display()))
        );
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "echo new\n");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750);
        assert_eq!(directory.entries(), ["link.sh", "script.sh"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn writes_file_in_read_only_folder() {
        let directory = TemporaryDir::new("write-read-only");
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "old").unwrap();
        // The file stays writable, but no file can be added next to it.
        directory.set_mode(0o500);
        let input = json!({ "path": path, "content": "new" });
        let result = call("write", &input).await;
        let content = std::fs::read_to_string(&path).unwrap();
        let entries = directory.entries();
        directory.set_mode(0o700);
        assert_eq!(result, Ok(format!("wrote {}", path.display())));
        assert_eq!(content, "new");
        assert_eq!(entries, ["file.txt"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn creates_target_of_dangling_symlink() {
        let directory = TemporaryDir::new("write-dangling");
        let link = directory.path().join("link.txt");
        std::os::unix::fs::symlink("target.txt", &link).unwrap();
        let input = json!({ "path": link, "content": "new" });
        assert_eq!(
            call("write", &input).await,
            Ok(format!("wrote {}", link.display()))
        );
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        let target = directory.path().join("target.txt");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(directory.entries(), ["link.txt", "target.txt"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_read_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TemporaryDir::new("write-read-only-file");
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let input = json!({ "path": path, "content": "new" });
        let result = call("write", &input).await;
        assert!(
            result.as_ref().is_err_and(
                |error| error.starts_with(&format!("failed to write {}: ", path.display()))
            ),
            "{result:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(directory.entries(), ["file.txt"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn writes_all_names_of_hard_linked_file() {
        let directory = TemporaryDir::new("write-hard-link");
        let path = directory.path().join("file.txt");
        let other = directory.path().join("other.txt");
        std::fs::write(&path, "old").unwrap();
        std::fs::hard_link(&path, &other).unwrap();
        let input = json!({ "path": path, "content": "new" });
        assert_eq!(
            call("write", &input).await,
            Ok(format!("wrote {}", path.display()))
        );
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "new");
        assert_eq!(directory.entries(), ["file.txt", "other.txt"]);
    }

    /// Needs the user to be in a second group, as most users are.
    #[cfg(unix)]
    #[tokio::test]
    async fn keeps_group_of_file() {
        use std::os::unix::fs::MetadataExt;
        let directory = TemporaryDir::new("write-group");
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "old").unwrap();
        let current = std::fs::metadata(&path).unwrap().gid();
        let mut groups = vec![0; 256];
        // SAFETY: the buffer holds as many groups as the length passed.
        let count = unsafe { libc::getgroups(groups.len() as i32, groups.as_mut_ptr()) };
        groups.truncate(count.max(0) as usize);
        let Some(&group) = groups.iter().find(|&&group| group != current) else {
            return;
        };
        std::os::unix::fs::chown(&path, None, Some(group)).unwrap();
        let input = json!({ "path": path, "content": "new" });
        assert_eq!(
            call("write", &input).await,
            Ok(format!("wrote {}", path.display()))
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(std::fs::metadata(&path).unwrap().gid(), group);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn writes_into_named_pipe() {
        use std::os::unix::fs::FileTypeExt;
        let fifo = TemporaryFile::fifo("write-fifo");
        let path = fifo.path().to_owned();
        // Opening the pipe waits for the other end, so read it on another thread.
        let reader = std::thread::spawn(move || std::fs::read_to_string(path).unwrap());
        let input = json!({ "path": fifo.path(), "content": "new" });
        let result = call("write", &input).await;
        let read = reader.join().unwrap();
        assert_eq!(result, Ok(format!("wrote {}", fifo.path().display())));
        assert_eq!(read, "new");
        let file_type = std::fs::symlink_metadata(fifo.path()).unwrap().file_type();
        assert!(file_type.is_fifo());
    }
}
