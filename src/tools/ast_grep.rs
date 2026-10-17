use super::{MAX_OUTPUT, argument, program, tool, truncate};
use serde_json::{Value, json};
use std::{process::Stdio, sync::OnceLock};
use tokio::{io::AsyncReadExt, process::Command};

pub const NAME: &str = "ast_grep";
pub(super) const PROGRAM: &str = "ast-grep";
pub(super) const GUIDANCE: &str = "Use ast_grep to find code by its syntax, such as calls, functions or types, and to outline a file before reading all of it.";

pub(super) fn installed() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| program::installed(PROGRAM))
}

pub(super) fn definition() -> Value {
    tool(
        NAME,
        "Search code by syntax with ast-grep, or outline its structure. search: find code matching pattern, written as code in the target language, where $NAME matches one node and $$$ matches any number of nodes. The pattern must cover a whole node, including modifiers such as pub. Returns path:line:text for each matched line. outline: list the top-level items of a file with their line numbers, or the exported items of each file in a folder",
        &json!({
            "command": { "type": "string", "enum": ["search", "outline"] },
            "pattern": { "type": "string", "description": "Code pattern to search for. Required for search" },
            "path": { "type": "string", "description": "File or folder. Defaults to the cwd" },
            "lang": { "type": "string", "description": "Language, such as rust or typescript. Defaults to the one each file's extension suggests" }
        }),
        &["command"],
    )
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let command = argument(input, "command")?;
    let mut arguments = match command {
        "search" => vec![
            "run".to_string(),
            "--heading=never".into(),
            format!("--pattern={}", argument(input, "pattern")?),
        ],
        "outline" => vec!["outline".to_string()],
        _ => return Err("command must be search or outline".into()),
    };
    arguments.push("--color=never".into());
    if let Some(lang) = input["lang"].as_str() {
        arguments.push(format!("--lang={lang}"));
    }
    arguments.extend(["--".into(), input["path"].as_str().unwrap_or(".").into()]);
    let failed = |error: std::io::Error| format!("failed to run {PROGRAM}: {error}");
    let mut child = Command::new(PROGRAM)
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(failed)?;
    let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(format!("failed to capture {PROGRAM} output"));
    };
    let errors = tokio::spawn(async move {
        let mut errors = Vec::new();
        let _ = stderr.read_to_end(&mut errors).await;
        errors
    });
    let mut output = Vec::new();
    stdout
        .take(MAX_OUTPUT as u64 + 1)
        .read_to_end(&mut output)
        .await
        .map_err(failed)?;
    let stdout = String::from_utf8_lossy(&output);
    if output.len() > MAX_OUTPUT {
        return Ok(truncate(stdout.into_owned()));
    }
    let status = child.wait().await.map_err(failed)?;
    let errors = errors.await.unwrap_or_default();
    let stderr = String::from_utf8_lossy(&errors);
    let stderr = stderr.trim_end();
    let text = match (stdout.is_empty(), stderr.is_empty()) {
        (_, true) => stdout.into_owned(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}{stderr}"),
    };
    if status.success() {
        return Ok(truncate(text));
    }
    if command == "search" && text.is_empty() {
        return Ok("no matches".into());
    }
    Err(truncate(format!("{text}\n{status}")))
}

#[cfg(test)]
mod tests;
