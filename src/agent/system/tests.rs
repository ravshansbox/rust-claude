use super::load_instructions_from;
use crate::tools::test_support::TemporaryDir;
use std::path::Path;

fn loaded(home: Option<&Path>, cwd: &Path) -> Vec<(String, String)> {
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
    assert_eq!(loaded(Some(home.path()), project.path()), Vec::new());
}
