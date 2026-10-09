use std::{
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

const MAX_OUTPUT: usize = 20_000;
const OUTPUT_GRACE: Duration = Duration::from_millis(100);

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

async fn read_capped(mut reader: impl tokio::io::AsyncRead + Unpin, output: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0; 8192];
    while let Ok(count) = reader.read(&mut chunk).await
        && count > 0
    {
        if let Ok(mut output) = output.lock() {
            let room = (MAX_OUTPUT + 1).saturating_sub(output.len());
            output.extend_from_slice(&chunk[..count.min(room)]);
        }
    }
}

fn snapshot(output: &Mutex<Vec<u8>>) -> Vec<u8> {
    output
        .lock()
        .map(|output| output.clone())
        .unwrap_or_default()
}

fn output_text(stdout_output: &Mutex<Vec<u8>>, stderr_output: &Mutex<Vec<u8>>) -> String {
    let mut text = String::from_utf8_lossy(&snapshot(stdout_output)).into_owned();
    let stderr = String::from_utf8_lossy(&snapshot(stderr_output)).into_owned();
    if !stderr.is_empty() {
        text.push_str("\nstderr:\n");
        text.push_str(&stderr);
    }
    truncate(text)
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

#[derive(Default)]
pub struct ReadGroup {
    paths: Vec<(String, usize)>,
}

impl ReadGroup {
    pub fn add(&mut self, path: String) {
        match self
            .paths
            .iter_mut()
            .find(|(existing, _)| *existing == path)
        {
            Some((_, count)) => *count += 1,
            None => self.paths.push((path, 1)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn summary(&self) -> String {
        self.paths
            .iter()
            .map(|(path, count)| match count {
                1 => path.clone(),
                _ => format!("{path} ({count})"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

const WRITE_PREVIEW_LINES: usize = 10;

pub fn diff(name: &str, input: &Value) -> Option<String> {
    if name == "write" {
        return write_preview(input["content"].as_str()?);
    }
    if name != "edit" {
        return None;
    }
    let old_text = input["old_text"].as_str()?;
    let new_text = input["new_text"].as_str()?;
    let diff = similar::TextDiff::from_lines(old_text, new_text);
    let lines: Vec<String> = diff
        .iter_all_changes()
        .map(|change| {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => '-',
                similar::ChangeTag::Insert => '+',
                similar::ChangeTag::Equal => ' ',
            };
            format!(
                "{sign}{}",
                change.value().trim_end_matches('\n').trim_start()
            )
        })
        .collect();
    Some(lines.join("\n"))
}

fn write_preview(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let mut preview: Vec<String> = lines
        .iter()
        .take(WRITE_PREVIEW_LINES)
        .map(|line| format!(" {}", line.trim_start()))
        .collect();
    let remaining = lines.len().saturating_sub(WRITE_PREVIEW_LINES);
    if remaining > 0 {
        let noun = if remaining == 1 { "line" } else { "lines" };
        preview.push(format!("… {remaining} more {noun}"));
    }
    Some(preview.join("\n"))
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
}

pub async fn call(name: &str, input: &Value) -> Result<String, String> {
    match name {
        "bash" => {
            let timeout = match &input["timeout"] {
                Value::Null => None,
                value => match value.as_u64() {
                    Some(seconds) if seconds > 0 => Some(seconds),
                    _ => return Err("timeout must be at least 1".into()),
                },
            };
            let mut command = tokio::process::Command::new("bash");
            command
                .args(["-c", argument(input, "command")?])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            let mut child = command
                .spawn()
                .map_err(|error| format!("failed to run command: {error}"))?;
            let mut process_group = ProcessGroup(child.id());
            let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
                return Err("failed to capture command output".into());
            };
            let stdout_output = Arc::new(Mutex::new(Vec::new()));
            let stderr_output = Arc::new(Mutex::new(Vec::new()));
            let readers = [
                tokio::spawn(read_capped(stdout, stdout_output.clone())),
                tokio::spawn(read_capped(stderr, stderr_output.clone())),
            ];
            let run = child.wait();
            let status = match timeout {
                Some(seconds) => tokio::time::timeout(Duration::from_secs(seconds), run)
                    .await
                    .map_err(|_| seconds),
                None => Ok(run.await),
            };
            match status {
                Ok(_) => process_group.0 = None,
                Err(_) => drop(process_group),
            }
            let _ = tokio::time::timeout(OUTPUT_GRACE, futures::future::join_all(readers)).await;
            let status = match status {
                Ok(status) => status.map_err(|error| format!("failed to run command: {error}"))?,
                Err(seconds) => {
                    let text = output_text(&stdout_output, &stderr_output);
                    let notice = format!("command timed out after {seconds}s");
                    return Err(if text.is_empty() {
                        notice
                    } else {
                        format!("{text}\n{notice}")
                    });
                }
            };
            let mut text = output_text(&stdout_output, &stderr_output);
            if !status.success() {
                text.push_str(&format!("\n{status}"));
            }
            Ok(text)
        }
        "read" => {
            let path = argument(input, "path")?;
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            let offset = match &input["offset"] {
                Value::Null => 1,
                value => match value.as_u64() {
                    Some(offset) => offset.max(1) as usize,
                    None => return Err("offset must be at least 1".into()),
                },
            };
            let limit = match &input["limit"] {
                Value::Null => usize::MAX,
                value => match value.as_u64() {
                    Some(limit) if limit > 0 => limit as usize,
                    _ => return Err("limit must be at least 1".into()),
                },
            };
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
        "edit" => {
            let path = argument(input, "path")?;
            let old_text = argument(input, "old_text")?;
            if old_text.is_empty() {
                return Err("old_text must not be empty".into());
            }
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|error| format!("failed to read {path}: {error}"))?;
            let mut old_text = old_text.to_string();
            let mut new_text = argument(input, "new_text")?.to_string();
            if count_matches(&content, &old_text) == 0
                && content.contains("\r\n")
                && !old_text.contains('\r')
            {
                old_text = old_text.replace('\n', "\r\n");
                new_text = new_text.replace("\r\n", "\n").replace('\n', "\r\n");
            }
            match count_matches(&content, &old_text) {
                1 => {}
                0 => return Err(format!("old_text not found in {path}")),
                count => return Err(format!("old_text matches {count} times in {path}")),
            }
            tokio::fs::write(path, content.replacen(&old_text, &new_text, 1))
                .await
                .map(|_| format!("edited {path}"))
                .map_err(|error| format!("failed to write {path}: {error}"))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_OUTPUT, ReadGroup, call, diff, read_lines};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    struct TemporaryFile(PathBuf);

    impl TemporaryFile {
        fn new(name: &str, content: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("rust-claude-{name}-{}", std::process::id()));
            std::fs::write(&path, content).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn content(&self) -> String {
            std::fs::read_to_string(&self.0).unwrap()
        }
    }

    impl Drop for TemporaryFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn times_out_bash_command() {
        let input = json!({ "command": "sleep 5", "timeout": 1 });
        assert_eq!(
            call("bash", &input).await,
            Err("command timed out after 1s".into())
        );
    }

    #[tokio::test]
    async fn keeps_output_when_bash_command_times_out() {
        let input = json!({ "command": "echo before; echo problem >&2; sleep 5", "timeout": 1 });
        assert_eq!(
            call("bash", &input).await,
            Err("before\n\nstderr:\nproblem\n\ncommand timed out after 1s".into())
        );
    }

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
        ];
        for (name, input, error) in cases {
            assert_eq!(call(name, &input).await, Err(error.into()), "{input}");
        }
    }

    #[tokio::test]
    async fn keeps_exit_status_when_output_is_truncated() {
        let input = json!({ "command": format!("head -c {} /dev/zero | tr '\\0' a; exit 3", MAX_OUTPUT * 2) });
        let text = call("bash", &input).await.unwrap();
        assert!(text.ends_with("exit status: 3"));
    }

    #[tokio::test]
    async fn keeps_only_start_of_long_output() {
        let input = json!({ "command": format!("head -c {} /dev/zero | tr '\\0' a; echo b >&2", MAX_OUTPUT * 50) });
        let text = call("bash", &input).await.unwrap();
        assert!(text.ends_with("… output truncated"));
        assert!(!text.contains("stderr"));
    }

    #[test]
    fn previews_first_lines_of_write() {
        let content: String = (1..=12).map(|number| format!("line {number}\n")).collect();
        let preview = diff("write", &json!({ "path": "a.txt", "content": content })).unwrap();
        let lines: Vec<&str> = preview.lines().collect();
        assert_eq!(lines.len(), 11);
        assert_eq!(lines[0], " line 1");
        assert_eq!(lines[9], " line 10");
        assert_eq!(lines[10], "… 2 more lines");
    }

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

    #[tokio::test]
    async fn reports_exit_status_once() {
        let input = json!({ "command": "echo hi; exit 2" });
        assert_eq!(
            call("bash", &input).await,
            Ok("hi\n\nexit status: 2".into())
        );
    }

    #[tokio::test]
    async fn returns_when_background_process_keeps_output_open() {
        let input = json!({ "command": "sleep 30 & echo $!", "timeout": 10 });
        let started = std::time::Instant::now();
        let text = call("bash", &input).await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let killed = std::process::Command::new("kill")
            .arg(text.trim())
            .status()
            .unwrap()
            .success();
        assert!(killed);
    }

    #[tokio::test]
    async fn kills_background_processes_on_timeout() {
        let pid_file = TemporaryFile::new("pid", "");
        let command = format!("sleep 30 & echo $! > {}; wait", pid_file.path().display());
        let input = json!({ "command": command, "timeout": 1 });
        assert!(call("bash", &input).await.is_err());
        let pid = pid_file.content();
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
        let file = TemporaryFile::new("edit", "");
        let input = json!({ "path": file.path(), "old_text": "", "new_text": "added" });
        assert_eq!(
            call("edit", &input).await,
            Err("old_text must not be empty".into())
        );
        assert_eq!(file.content(), "");
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

    #[test]
    fn cuts_single_long_line() {
        let content = format!("{}\nnext\n", "a".repeat(MAX_OUTPUT + 1));
        let text = read_lines(&content, 1, usize::MAX);
        assert!(text.ends_with("… output truncated, continue with offset 2"));
        assert_eq!(read_lines(&content, 2, usize::MAX), "next\n");
    }

    #[test]
    fn diffs_edit_input() {
        let input = json!({ "path": "a", "old_text": "a\nb\n", "new_text": "a\nc\n" });
        assert_eq!(diff("edit", &input), Some(" a\n-b\n+c".into()));
        assert_eq!(diff("write", &input), None);
    }

    #[test]
    fn trims_leading_spaces_in_previews() {
        let edit = json!({ "path": "a", "old_text": "    a\n\tb\n", "new_text": "    a\n  c\n" });
        assert_eq!(diff("edit", &edit), Some(" a\n-b\n+c".into()));
        let write = json!({ "path": "a", "content": "fn main() {\n    body\n}\n" });
        assert_eq!(
            diff("write", &write),
            Some(" fn main() {\n body\n }".into())
        );
    }

    #[test]
    fn groups_reads_with_counts() {
        let mut group = ReadGroup::default();
        for path in ["a.rs", "b.rs", "a.rs", "a.rs"] {
            group.add(path.into());
        }
        assert_eq!(group.summary(), "a.rs (3), b.rs");
    }
}
