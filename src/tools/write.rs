use super::argument;
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
    tokio::fs::write(path, content)
        .await
        .map(|_| format!("wrote {path}"))
        .map_err(|error| format!("failed to write {path}: {error}"))
}

#[cfg(test)]
mod tests {
    use crate::tools::call;
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
}
