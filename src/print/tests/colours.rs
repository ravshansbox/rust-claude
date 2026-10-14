use super::super::Printer;
use super::without_colours;
use crate::agent::test_support::{self, MockApi, text_reply, tool_reply};
use serde_json::json;
use std::io;

#[tokio::test]
async fn uses_github_high_contrast_colours() {
    for (dark, keyword) in [(true, "255;148;146"), (false, "160;17;31")] {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                &json!({ "command": "python3 - <<'EOF'\ndef f():\n    pass\nEOF" }),
            ),
            text_reply("done"),
        ])
        .await;
        let mut err = Vec::new();
        let mut printer = Printer::new(io::sink(), true, &mut err, true, dark, None);
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        let finished = printer.finish(result);
        finished.unwrap();
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.contains(&format!("\x1b[38;2;{keyword}mdef")),
            "dark {dark}: {err:?}"
        );
    }
}

#[tokio::test]
async fn highlights_bash_commands() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "bash",
            &json!({ "command": "echo \"$HOME\" | wc -c" }),
        ),
        text_reply("done"),
    ])
    .await;
    let mut err = Vec::new();
    let mut printer = Printer::new(io::sink(), true, &mut err, true, true, None);
    let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
    let finished = printer.finish(result);
    finished.unwrap();
    let err = String::from_utf8(err).unwrap();
    let plain = without_colours(&err);
    assert!(
        plain.contains(" bash  echo \"$HOME\" | wc -c\n"),
        "{plain:?}"
    );
    let command = err.lines().find(|line| line.contains("echo")).unwrap();
    assert!(command.matches("\x1b[38;2;").count() > 1, "{command:?}");
}

#[tokio::test]
async fn fills_tool_calls_with_a_background_near_the_terminal_one() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "echo hi" })),
        text_reply("done"),
    ])
    .await;
    let mut err = Vec::new();
    let mut printer = Printer::new(io::sink(), true, &mut err, true, true, Some((30, 30, 30)));
    let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
    printer.finish(result).unwrap();
    let err = String::from_utf8(err).unwrap();
    let lines: Vec<&str> = err.lines().take(1).collect();
    assert!(
        lines[0].starts_with("\x1b[0m\x1b[48;2;50;50;50m\x1b[38;2;220;220;220m\x1b[1m bash "),
        "{lines:?}"
    );
    for line in &lines {
        assert!(
            line.ends_with("\x1b[48;2;42;42;42m\x1b[K\x1b[0m"),
            "{lines:?}"
        );
    }
    assert_eq!(
        without_colours(&err).lines().take(1).collect::<Vec<_>>(),
        [" bash  echo hi"]
    );
}
