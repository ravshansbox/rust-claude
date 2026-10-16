use super::{INPUT_VARIABLE, Restart};
use std::{ffi::OsStr, path::PathBuf};

fn restart(session: Option<&str>, config_dir: Option<&str>) -> Restart {
    Restart {
        program: PathBuf::from("/bin/rust-coding-agent"),
        session: session.map(Into::into),
        model: "claude-x".into(),
        effort: "high",
        input: "half a prompt".into(),
        config_dir: config_dir.map(PathBuf::from),
    }
}

fn arguments(restart: &Restart) -> Vec<String> {
    restart
        .command()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn starts_the_installed_program_with_the_session_model_and_effort() {
    let restart = restart(Some("abc-123"), Some("/config"));
    let command = restart.command();
    assert_eq!(command.get_program(), "/bin/rust-coding-agent");
    assert_eq!(
        arguments(&restart),
        [
            "--config-dir",
            "/config",
            "--resume",
            "abc-123",
            "--model",
            "claude-x",
            "--effort",
            "high",
        ]
    );
    assert!(
        command
            .get_envs()
            .any(|(name, value)| name == INPUT_VARIABLE
                && value == Some(OsStr::new("half a prompt")))
    );
}

#[test]
fn starts_a_new_session_when_nothing_was_saved() {
    assert_eq!(
        arguments(&restart(None, None)),
        ["--model", "claude-x", "--effort", "high"]
    );
}
