use super::{argument, program, tool, truncate};
use serde_json::{Value, json};
use std::{process::Stdio, sync::OnceLock};
use tokio::process::Command;

pub const NAME: &str = "ast_grep";
const PROGRAM: &str = "ast-grep";
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
            "run",
            "--heading",
            "never",
            "--pattern",
            argument(input, "pattern")?,
        ],
        "outline" => vec!["outline"],
        _ => return Err("command must be search or outline".into()),
    };
    arguments.extend(["--color", "never"]);
    if let Some(lang) = input["lang"].as_str() {
        arguments.extend(["--lang", lang]);
    }
    arguments.extend(["--", input["path"].as_str().unwrap_or(".")]);
    let output = Command::new(PROGRAM)
        .args(&arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("failed to run {PROGRAM}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim_end();
    let text = match (stdout.is_empty(), stderr.is_empty()) {
        (_, true) => stdout.into_owned(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}{stderr}"),
    };
    if output.status.success() {
        return Ok(truncate(text));
    }
    if command == "search" && text.is_empty() {
        return Ok("no matches".into());
    }
    Err(truncate(format!("{text}\n{}", output.status)))
}

#[cfg(test)]
mod tests;
