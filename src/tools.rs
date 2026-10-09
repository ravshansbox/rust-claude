use std::{process::Stdio, time::Duration};

use serde_json::{Value, json};

const MAX_OUTPUT: usize = 20_000;

struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.0 {
            unsafe {
                libc::killpg(id as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… output truncated");
    }
    text
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

fn read_lines(content: &str, offset: usize, limit: usize) -> String {
    let mut text = String::new();
    let lines = content
        .split_inclusive('\n')
        .enumerate()
        .skip(offset - 1)
        .take(limit);
    for (index, line) in lines {
        if text.len() + line.len() > MAX_OUTPUT {
            let next_line = if text.is_empty() {
                text = truncate(line.to_string());
                index + 2
            } else {
                text.push_str("… output truncated");
                index + 1
            };
            text.push_str(&format!(", continue with offset {next_line}"));
            return text;
        }
        text.push_str(line);
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
                "properties": {
                    "command": { "type": "string" },
                    "timeout": { "type": "integer", "description": "Seconds before the command is killed. No limit when omitted" }
                },
                "required": ["command"]
            }
        },
        {
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
            let mut command = tokio::process::Command::new("bash");
            command
                .args(["-lc", argument(input, "command")?])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            let child = command
                .spawn()
                .map_err(|error| format!("failed to run command: {error}"))?;
            let mut process_group = ProcessGroup(child.id());
            let run = child.wait_with_output();
            let output = match input["timeout"].as_u64() {
                Some(seconds) => tokio::time::timeout(Duration::from_secs(seconds), run)
                    .await
                    .map_err(|_| format!("command timed out after {seconds}s"))?,
                None => run.await,
            }
            .map_err(|error| format!("failed to run command: {error}"))?;
            process_group.0 = None;

            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                text.push_str("\nstderr:\n");
                text.push_str(&stderr);
            }
            let mut text = truncate(text);
            if !output.status.success() {
                text.push_str(&format!("\nexit status: {}", output.status));
            }
            Ok(text)
        }
        "read" => {
            let path = argument(input, "path")?;
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
            let limit = input["limit"]
                .as_u64()
                .map_or(usize::MAX, |limit| limit as usize);
            let line_count = content.split_inclusive('\n').count();
            if offset > line_count.max(1) {
                return Err(format!(
                    "offset {offset} is past the end of {path}, which has {line_count} lines"
                ));
            }
            Ok(read_lines(&content, offset, limit))
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
            if old_text.is_empty() {
                return Err("old_text must not be empty".into());
            }
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            match count_matches(&content, old_text) {
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

#[cfg(test)]
mod tests {
    use super::{MAX_OUTPUT, call, read_lines};
    use serde_json::json;

    #[tokio::test]
    async fn times_out_bash_command() {
        let input = json!({ "command": "sleep 5", "timeout": 1 });
        assert_eq!(
            call("bash", &input).await,
            Err("command timed out after 1s".into())
        );
    }

    #[tokio::test]
    async fn keeps_exit_status_when_output_is_truncated() {
        let input = json!({ "command": format!("head -c {} /dev/zero | tr '\\0' a; exit 3", MAX_OUTPUT * 2) });
        let text = call("bash", &input).await.unwrap();
        assert!(text.ends_with("exit status: 3"));
    }

    #[tokio::test]
    async fn kills_background_processes_on_timeout() {
        let pid_file =
            std::env::temp_dir().join(format!("rust-claude-test-{}", std::process::id()));
        let command = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let input = json!({ "command": command, "timeout": 1 });
        assert!(call("bash", &input).await.is_err());
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        std::fs::remove_file(&pid_file).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success();
        assert!(!alive);
    }

    #[test]
    fn reads_requested_lines() {
        let content = "one\ntwo\nthree\nfour\n";
        assert_eq!(read_lines(content, 1, usize::MAX), content);
        assert_eq!(read_lines(content, 2, 2), "two\nthree\n");
        assert_eq!(read_lines(content, 10, usize::MAX), "");
    }

    #[test]
    fn cuts_long_output_at_whole_lines() {
        let line = format!("{}\n", "a".repeat(99));
        let content = line.repeat(MAX_OUTPUT / 100 + 10);
        let text = read_lines(&content, 1, usize::MAX);
        let next_line = MAX_OUTPUT / 100 + 1;
        assert!(text.starts_with(&line.repeat(MAX_OUTPUT / 100)));
        assert!(text.ends_with(&format!(
            "… output truncated, continue with offset {next_line}"
        )));
    }

    #[test]
    fn truncates_inside_multibyte_character() {
        let text = super::truncate(format!("a{}", "é".repeat(MAX_OUTPUT)));
        assert!(text.ends_with("\n… output truncated"));
        assert_eq!(text.len(), MAX_OUTPUT - 1 + "\n… output truncated".len());
    }

    #[tokio::test]
    async fn rejects_empty_old_text() {
        let path = std::env::temp_dir().join(format!("rust-claude-edit-{}", std::process::id()));
        std::fs::write(&path, "").unwrap();
        let input = json!({ "path": path, "old_text": "", "new_text": "added" });
        let result = call("edit", &input).await;
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(result, Err("old_text must not be empty".into()));
        assert_eq!(content, "");
    }

    #[tokio::test]
    async fn rejects_offset_past_end_of_file() {
        let path = std::env::temp_dir().join(format!("rust-claude-offset-{}", std::process::id()));
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let past_end = call("read", &json!({ "path": path, "offset": 3 })).await;
        let last_line = call("read", &json!({ "path": path, "offset": 2 })).await;
        std::fs::remove_file(&path).unwrap();
        assert!(past_end.unwrap_err().contains("which has 2 lines"));
        assert_eq!(last_line, Ok("two\n".into()));
    }

    #[tokio::test]
    async fn rejects_overlapping_old_text() {
        let path = std::env::temp_dir().join(format!("rust-claude-overlap-{}", std::process::id()));
        std::fs::write(&path, "aaa").unwrap();
        let input = json!({ "path": path, "old_text": "aa", "new_text": "b" });
        let result = call("edit", &input).await;
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().contains("matches 2 times"));
        assert_eq!(content, "aaa");
    }

    #[test]
    fn cuts_single_long_line() {
        let content = format!("{}\nnext\n", "a".repeat(MAX_OUTPUT + 1));
        let text = read_lines(&content, 1, usize::MAX);
        assert!(text.ends_with("… output truncated, continue with offset 2"));
        assert_eq!(read_lines(&content, 2, usize::MAX), "next\n");
    }
}
