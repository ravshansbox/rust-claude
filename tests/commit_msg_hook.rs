#![cfg(unix)]

use std::process::{Command, Output};

fn run_hook(name: &str, message: &str) -> Output {
    let path = std::env::temp_dir().join(format!(
        "rust-coding-agent-commit-msg-{name}-{}",
        std::process::id()
    ));
    std::fs::write(&path, message).unwrap();
    let output = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.githooks/commit-msg"))
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    output
}

#[test]
fn accepts_conventional_messages() {
    for (name, message) in [
        ("feat", "feat: add a thing\n"),
        ("scope", "fix(tui): hide notice\n"),
        ("breaking", "refactor!: rename stop tool\n"),
        ("scoped-breaking", "feat(mcp)!: drop old config\n"),
        (
            "comment",
            "# Please enter the commit message\ndocs: update README\n",
        ),
    ] {
        let output = run_hook(name, message);
        assert!(
            output.status.success(),
            "{message:?} was rejected: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn rejects_other_messages() {
    for (name, message) in [
        ("plain", "add a thing\n"),
        ("unknown-type", "feature: add a thing\n"),
        ("no-space", "feat:add a thing\n"),
        ("no-subject", "feat: \n"),
        ("upper-case", "Feat: add a thing\n"),
        ("empty-scope", "feat(): add a thing\n"),
    ] {
        let output = run_hook(name, message);
        assert!(!output.status.success(), "{message:?} was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("conventional commit"),
            "no hint for {message:?}"
        );
    }
}
