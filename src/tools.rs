use serde_json::{Value, json};

const MAX_OUTPUT: usize = 20_000;

fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        text.truncate(MAX_OUTPUT);
        while !text.is_char_boundary(text.len()) {
            text.pop();
        }
        text.push_str("\n… output truncated");
    }
    text
}

pub fn definitions() -> Value {
    json!([
        {
            "name": "bash",
            "description": "Run a bash command in the current project and return stdout and stderr",
            "input_schema": {
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"]
            }
        },
        {
            "name": "read",
            "description": "Read a UTF-8 file from the current project",
            "input_schema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        },
        {
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
        },
        {
            "name": "edit",
            "description": "Replace text in a UTF-8 file. old_text must match exactly once",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" }
                },
                "required": ["path", "old_text", "new_text"]
            }
        }
    ])
}

pub fn summary(name: &str, input: &Value) -> String {
    let key = match name {
        "bash" => "command",
        "read" | "write" | "edit" => "path",
        _ => return input.to_string(),
    };
    input[key].as_str().unwrap_or_default().to_string()
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
}

pub async fn call(name: &str, input: &Value) -> Result<String, String> {
    match name {
        "bash" => {
            let output = tokio::process::Command::new("bash")
                .args(["-lc", argument(input, "command")?])
                .kill_on_drop(true)
                .output()
                .await
                .map_err(|error| format!("failed to run command: {error}"))?;

            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                text.push_str("\nstderr:\n");
                text.push_str(&stderr);
            }
            if !output.status.success() {
                text.push_str(&format!("\nexit status: {}", output.status));
            }
            Ok(truncate(text))
        }
        "read" => {
            let path = argument(input, "path")?;
            tokio::fs::read_to_string(path)
                .await
                .map(truncate)
                .map_err(|error| format!("failed to read {path}: {error}"))
        }
        "write" => {
            let path = argument(input, "path")?;
            tokio::fs::write(path, argument(input, "content")?)
                .await
                .map(|_| format!("wrote {path}"))
                .map_err(|error| format!("failed to write {path}: {error}"))
        }
        "edit" => {
            let path = argument(input, "path")?;
            let old_text = argument(input, "old_text")?;
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            match content.matches(old_text).count() {
                1 => {}
                0 => return Err(format!("old_text not found in {path}")),
                count => return Err(format!("old_text matches {count} times in {path}")),
            }
            tokio::fs::write(
                path,
                content.replacen(old_text, argument(input, "new_text")?, 1),
            )
            .await
            .map(|_| format!("edited {path}"))
            .map_err(|error| format!("failed to write {path}: {error}"))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}
