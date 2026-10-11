use std::path::{Path, PathBuf};

pub(super) const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

pub(super) const SYSTEM_PROMPT: &str = r#"You are rust-coding-agent, a small coding agent running in a terminal.
Use your tools to inspect and change the project in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely.
Use plain English (ISO 24495-1)."#;

const INSTRUCTIONS_FILE: &str = "AGENTS.md";

#[derive(Clone)]
pub struct Instructions {
    pub label: String,
    pub(super) text: String,
}

pub(super) fn load_instructions() -> Vec<Instructions> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if let Some(home) = &home {
        candidates.push((
            format!("~/{INSTRUCTIONS_FILE}"),
            home.join(INSTRUCTIONS_FILE),
        ));
    }
    let same_dir = match (&home, &cwd) {
        (Some(home), Some(cwd)) => same_path(home, cwd),
        _ => false,
    };
    if !same_dir {
        candidates.push((
            format!("./{INSTRUCTIONS_FILE}"),
            PathBuf::from(INSTRUCTIONS_FILE),
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
mod tests {
    use super::SYSTEM_PROMPT;

    #[test]
    fn leaves_search_and_question_rules_out_of_the_system_prompt() {
        let text = SYSTEM_PROMPT;
        assert!(text.contains("Use plain English (ISO 24495-1)."));
        assert!(!text.contains("ast-grep"));
        assert!(!text.contains("ripgrep"));
        assert!(!text.contains("bold"));
        assert!(!text.contains("Letter the options"));
    }
}
