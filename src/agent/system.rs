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
mod tests {
    use super::load_instructions_from;
    use crate::tools::test_support::TemporaryDir;

    fn loaded(home: Option<&std::path::Path>, cwd: &std::path::Path) -> Vec<(String, String)> {
        load_instructions_from(home, cwd)
            .into_iter()
            .map(|instructions| (instructions.label, instructions.text))
            .collect()
    }

    #[test]
    fn loads_home_instructions_before_the_current_folder_ones() {
        let home = TemporaryDir::new("instructions-home");
        let project = TemporaryDir::new("instructions-project");
        std::fs::write(home.path().join("AGENTS.md"), "home rules").unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "project rules").unwrap();
        assert_eq!(
            loaded(Some(home.path()), project.path()),
            [
                ("~/AGENTS.md".to_string(), "home rules".to_string()),
                ("./AGENTS.md".to_string(), "project rules".to_string()),
            ]
        );
    }

    #[test]
    fn loads_instructions_once_when_run_from_the_home_folder() {
        let home = TemporaryDir::new("instructions-same");
        std::fs::write(home.path().join("AGENTS.md"), "home rules").unwrap();
        assert_eq!(
            loaded(Some(home.path()), &home.path().join(".")),
            [("~/AGENTS.md".to_string(), "home rules".to_string())]
        );
    }

    #[test]
    fn skips_blank_and_missing_instructions() {
        let home = TemporaryDir::new("instructions-blank");
        let project = TemporaryDir::new("instructions-missing");
        std::fs::write(home.path().join("AGENTS.md"), " \n\n").unwrap();
        assert!(loaded(Some(home.path()), project.path()).is_empty());
    }
}
