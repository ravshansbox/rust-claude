use std::path::{Path, PathBuf};

pub(super) const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

pub(super) const SYSTEM_PROMPT: &str =
    "Use your tools to inspect and change files in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely.
Use plain English (ISO 24495-1).";

const INSTRUCTIONS_FILE: &str = "AGENTS.md";

#[derive(Clone)]
pub struct Instructions {
    pub label: String,
    pub(super) text: String,
}

pub(super) fn load_instructions() -> Vec<Instructions> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().unwrap_or_default();
    load_instructions_from(home.as_deref(), &cwd)
}

fn load_instructions_from(home: Option<&Path>, cwd: &Path) -> Vec<Instructions> {
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push((
            format!("~/{INSTRUCTIONS_FILE}"),
            home.join(INSTRUCTIONS_FILE),
        ));
    }
    if !home.is_some_and(|home| same_path(home, cwd)) {
        candidates.push((
            format!("./{INSTRUCTIONS_FILE}"),
            cwd.join(INSTRUCTIONS_FILE),
        ));
    }

    candidates
        .into_iter()
        .filter_map(|(label, path)| {
            let text = std::fs::read_to_string(path).ok()?;
            (!text.trim().is_empty()).then_some(Instructions { label, text })
        })
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests;
