use std::path::{Path, PathBuf};

pub(super) const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

const SYSTEM_PROMPT: &str = r#"You are rust-claude, a small coding agent running in a terminal.
Use your tools to inspect and change the project in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely.
Prefer edit and write over bash for changing files.
Put questions to the user in bold."#;

const SEARCH_PROGRAMS: [&str; 2] = ["ast-grep", "rg"];

pub(super) fn system_text(missing: &[&str]) -> String {
    let search = match (missing.contains(&"ast-grep"), missing.contains(&"rg")) {
        (false, false) => {
            "Search code with ast-grep. Fall back to ripgrep for plain text, comments, strings and files ast-grep cannot parse."
        }
        (true, false) => "Search with ripgrep.",
        (false, true) => "Search code with ast-grep.",
        (true, true) => return SYSTEM_PROMPT.to_string(),
    };
    format!("{SYSTEM_PROMPT}\n{search}")
}

pub(super) fn missing_search_programs(path: Option<std::ffi::OsString>) -> Vec<&'static str> {
    let directories: Vec<_> = path
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    SEARCH_PROGRAMS
        .into_iter()
        .filter(|program| {
            let file = format!("{program}{}", std::env::consts::EXE_SUFFIX);
            !directories
                .iter()
                .any(|directory| directory.join(&file).is_file())
        })
        .collect()
}

const INSTRUCTIONS_FILE: &str = "AGENTS.md";

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
    use super::{missing_search_programs, system_text};

    #[test]
    fn tells_the_model_to_prefer_ast_grep_for_code_search() {
        assert!(system_text(&[]).contains(
            "Search code with ast-grep. Fall back to ripgrep for plain text, comments, strings and files ast-grep cannot parse."
        ));
    }

    #[test]
    fn leaves_missing_search_programs_out_of_the_system_prompt() {
        let without_ast_grep = system_text(&["ast-grep"]);
        assert!(without_ast_grep.contains("Search with ripgrep."));
        assert!(!without_ast_grep.contains("ast-grep"));
        let without_ripgrep = system_text(&["rg"]);
        assert!(without_ripgrep.contains("Search code with ast-grep."));
        assert!(!without_ripgrep.contains("ripgrep"));
        let without_both = system_text(&["ast-grep", "rg"]);
        assert!(!without_both.contains("ast-grep"));
        assert!(!without_both.contains("ripgrep"));
        assert!(without_both.contains("Put questions to the user in bold."));
    }

    #[test]
    fn finds_search_programs_missing_from_path() {
        let directory =
            std::env::temp_dir().join(format!("rust-claude-path-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("rg{}", std::env::consts::EXE_SUFFIX)),
            "",
        )
        .unwrap();
        let path = std::env::join_paths([&directory]).unwrap();
        assert_eq!(missing_search_programs(Some(path)), vec!["ast-grep"]);
        assert_eq!(missing_search_programs(None), vec!["ast-grep", "rg"]);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
