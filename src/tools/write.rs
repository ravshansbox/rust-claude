use super::{argument, replace_file, tool};
use serde_json::{Value, json};
use std::io;

pub(super) fn definition() -> Value {
    tool(
        "write",
        "Create or replace a UTF-8 file in the cwd",
        &json!({
            "path": { "type": "string" },
            "content": { "type": "string" }
        }),
        &["path", "content"],
    )
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let path = argument(input, "path")?;
    let content = argument(input, "content")?;
    if let Ok(metadata) = tokio::fs::metadata(path).await
        && !metadata.is_file()
    {
        return Err(format!("{path} is not a regular file"));
    }
    let failed = |error: io::Error| format!("failed to write {path}: {error}");
    if let Some(parent) = std::path::Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    replace_file(path, content.to_string(), None)
        .await
        .map(|()| format!("wrote {path}"))
        .map_err(failed)
}

#[cfg(test)]
mod tests;
