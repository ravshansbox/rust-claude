use super::{MAX_OUTPUT, argument, number, program, tool, truncate};
use serde_json::{Value, json};
use std::{fmt::Write, process::Stdio, sync::OnceLock};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};

pub const NAME: &str = "grep";
pub(super) const PROGRAM: &str = "rg";
const DEFAULT_LIMIT: u64 = 100;
pub(super) const GUIDANCE: &str =
    "Use grep to search file contents for text, such as strings, comments or names.";

pub(super) fn installed() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| program::installed(PROGRAM))
}

pub(super) fn definition() -> Value {
    tool(
        NAME,
        "Search file contents for a regular expression with ripgrep. Skips files that .gitignore lists, hidden files and binary files. Returns path:line:text for each matching line, and path-line-text for lines of context, sorted by path",
        &json!({
            "pattern": { "type": "string", "description": "Regular expression, in Rust regex syntax" },
            "path": { "type": "string", "description": "File or folder. Defaults to the cwd" },
            "glob": { "type": "string", "description": "Only search files matching this glob, such as *.rs. Start with ! to leave files out" },
            "ignore_case": { "type": "boolean", "description": "Match upper and lower case alike" },
            "literal": { "type": "boolean", "description": "Treat pattern as plain text, not a regular expression" },
            "context": { "type": "integer", "description": "Lines to show before and after each match" },
            "limit": { "type": "integer", "description": "Maximum number of matches. Default: 100" }
        }),
        &["pattern"],
    )
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let pattern = argument(input, "pattern")?;
    let limit = number(input, "limit", 1)?.unwrap_or(DEFAULT_LIMIT);
    let context = number(input, "context", 0)?;
    let mut command = Command::new(PROGRAM);
    command.args(["--json", "--sort", "path"]);
    if input["ignore_case"] == true {
        command.arg("--ignore-case");
    }
    if input["literal"] == true {
        command.arg("--fixed-strings");
    }
    if let Some(glob) = input["glob"].as_str() {
        command.args(["--glob", glob]);
    }
    if let Some(context) = context {
        command.args(["--context", &context.to_string()]);
    }
    command
        .args(["--", pattern, input["path"].as_str().unwrap_or(".")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let failed = |error: std::io::Error| format!("failed to run {PROGRAM}: {error}");
    let mut child = command.spawn().map_err(failed)?;
    let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(format!("failed to capture {PROGRAM} output"));
    };
    let errors = tokio::spawn(async move {
        let mut errors = String::new();
        let _ = stderr.read_to_string(&mut errors).await;
        errors
    });
    let mut events = BufReader::new(stdout).lines();
    let mut text = String::new();
    let mut matches = 0;
    let mut last: Option<(String, u64)> = None;
    let mut stopped = false;
    while let Some(event) = events.next_line().await.map_err(failed)? {
        let Ok(event) = serde_json::from_str::<Value>(&event) else {
            continue;
        };
        let separator = match event["type"].as_str() {
            Some("match") => ':',
            Some("context") => '-',
            _ => continue,
        };
        let data = &event["data"];
        let path = data["path"]["text"].as_str().unwrap_or_default();
        let line = data["line_number"].as_u64().unwrap_or_default();
        let follows_last = last
            .as_ref()
            .is_some_and(|(last_path, last_line)| last_path == path && last_line + 1 == line);
        if matches == limit && (separator == ':' || !follows_last) {
            stopped = true;
            break;
        }
        if last.is_some() && context.is_some() && !follows_last {
            text.push_str("--\n");
        }
        let line_text = data["lines"]["text"].as_str().unwrap_or_default();
        let _ = write!(text, "{path}{separator}{line}{separator}{line_text}");
        if !text.ends_with('\n') {
            text.push('\n');
        }
        last = Some((path.to_string(), line));
        if separator == ':' {
            matches += 1;
        }
        if text.len() > MAX_OUTPUT {
            break;
        }
    }
    if stopped {
        let _ = write!(
            text,
            "… stopped at {limit} {}, use limit for more or refine the pattern",
            if limit == 1 { "match" } else { "matches" }
        );
        return Ok(truncate(text));
    }
    drop(events);
    let status = child.wait().await.map_err(failed)?;
    let errors = errors.await.unwrap_or_default();
    let errors = errors.trim_end();
    if text.is_empty() {
        return if status.code() == Some(1) && errors.is_empty() {
            Ok("no matches".into())
        } else {
            Err(truncate(format!("{errors}\n{status}")))
        };
    }
    if !errors.is_empty() {
        text.push_str(errors);
    }
    Ok(truncate(text))
}

#[cfg(test)]
mod tests;
