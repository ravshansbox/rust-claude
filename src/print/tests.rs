use super::{AgentEvent, Printer};
use crate::agent::test_support::{self, MockApi, Reply, text_reply, tool_reply};
use crate::tools::test_support::TemporaryDir;
use serde_json::json;
use std::io;

const ESCAPES: &str = "\x1b]52;c;aGk=\x07\x1b[2J\u{9b}2J\r";

/// Removes the colour codes print mode writes itself.
fn without_colours(text: &str) -> String {
    let mut plain = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("\x1b[") {
        plain.push_str(&rest[..start]);
        let code = &rest[start + 2..];
        match code.find(|c: char| !c.is_ascii_digit() && c != ';') {
            Some(end) if code[end..].starts_with(['m', 'K']) => rest = &code[end + 1..],
            _ => {
                plain.push_str("\x1b[");
                rest = code;
            }
        }
    }
    plain.push_str(rest);
    plain
}

fn has_controls(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
}

#[test]
fn separates_thinking_tool_calls_and_notices_with_blank_lines() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
    printer.event(AgentEvent::Thinking("let me".into()));
    printer.event(AgentEvent::Thinking("\tthink".into()));
    printer.event(AgentEvent::ToolStart {
        name: "bash".into(),
        summary: String::new(),
        diff: Some("ls".into()),
    });
    printer.event(AgentEvent::ToolDone {
        name: "bash".into(),
        error: Some("boom".into()),
        note: None,
    });
    printer.event(AgentEvent::Notice("note".into()));
    printer.event(AgentEvent::Text("answer".into()));
    printer.finish(Ok(())).unwrap();
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "let me\tthink\n\nbash\nls\n\nbash failed: boom\n\nnote\n"
    );
    assert_eq!(String::from_utf8(out).unwrap(), "answer\n");
}

#[test]
fn keeps_the_answer_line_out_of_a_separate_error_stream() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, true, &mut err, false, false, None);
    for (text, command) in [("I'll look.", "ls"), ("Now another.", "pwd")] {
        printer.event(AgentEvent::Text(text.into()));
        printer.event(AgentEvent::ToolStart {
            name: "bash".into(),
            summary: String::new(),
            diff: Some(command.into()),
        });
    }
    printer.event(AgentEvent::Text("Done.".into()));
    printer.finish(Ok(())).unwrap();
    assert_eq!(String::from_utf8(err).unwrap(), "bash\nls\n\nbash\npwd\n");
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "I'll look.\n\nNow another.\n\nDone.\n"
    );
}

#[test]
fn leaves_one_blank_line_after_thinking_that_ends_with_newlines() {
    let mut err = Vec::new();
    let mut printer = Printer::new(io::sink(), false, &mut err, false, false, None);
    printer.event(AgentEvent::Thinking("one\n\n".into()));
    printer.event(AgentEvent::Thinking("two.\n".into()));
    printer.event(AgentEvent::Thinking("\n".into()));
    printer.event(AgentEvent::Notice("note".into()));
    printer.event(AgentEvent::Thinking("last\n\n".into()));
    printer.finish(Err(anyhow::anyhow!("boom"))).unwrap_err();
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "one\n\ntwo.\n\nnote\n\nlast\n"
    );
}

#[test]
fn shows_thinking_in_grey_italics() {
    let mut err = Vec::new();
    let mut printer = Printer::new(io::sink(), true, &mut err, true, true, None);
    printer.event(AgentEvent::Thinking("hmm\x1b[2J".into()));
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "\x1b[90m\x1b[3mhmm[2J\x1b[0m"
    );
}

#[test]
fn leaves_one_blank_line_before_the_answer_on_a_shared_terminal() {
    struct Screen<'a>(&'a std::cell::RefCell<Vec<u8>>);
    impl io::Write for Screen<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let screen = std::cell::RefCell::new(Vec::new());
    let mut printer = Printer::new(Screen(&screen), true, Screen(&screen), true, true, None);
    printer.event(AgentEvent::Text("first".into()));
    printer.event(AgentEvent::Notice("note".into()));
    printer.event(AgentEvent::Text("second".into()));
    printer.finish(Ok(())).unwrap();
    drop(printer);
    let shown = without_colours(&String::from_utf8(screen.into_inner()).unwrap());
    assert_eq!(shown, "first\n\nnote\n\nsecond\n");
}

#[tokio::test]
async fn keeps_escape_sequences_from_the_model_and_tools_out_of_the_terminal() {
    let directory = TemporaryDir::new("print-escape");
    let path = directory.path().join("escape");
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "write",
            json!({ "path": path, "content": format!("one{ESCAPES}\ntwo\n") }),
        ),
        tool_reply(
            "call-2",
            "bash",
            json!({
                "command": format!("printf 'out\\033]52;c;aGk=\\007\\033[2J'; sleep 5 # {ESCAPES}"),
                "timeout": 1,
            }),
        ),
        text_reply(&format!("done{ESCAPES}")),
    ])
    .await;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, true, &mut err, true, true, None);
    let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
    printer.flush_reads();
    printer.finish(result).unwrap();
    let out = String::from_utf8(out).unwrap();
    let err = String::from_utf8(err).unwrap();

    assert_eq!(out, "done]52;c;aGk=[2J2J\n");
    assert!(err.contains("\x1b[38;2;"), "colours missing: {err:?}");
    let plain = without_colours(&err);
    assert!(!has_controls(&plain), "control characters in {plain:?}");
    assert!(plain.contains(" one]52;c;aGk=[2J2J"), "{plain:?}");
    assert!(plain.contains(" bash  printf"), "{plain:?}");
    assert!(plain.contains("bash failed: out]52;c;aGk=[2J"), "{plain:?}");
}

#[tokio::test]
async fn answers_each_background_command_when_it_finishes() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "bash",
            json!({ "command": "sleep 0.3; echo finished", "run_in_background": true }),
        ),
        text_reply("waiting"),
        text_reply("seen it"),
    ])
    .await;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    let mut checkpoint = None;
    let result = super::run(&mut agent, &mut printer, "go", &[], &mut checkpoint).await;
    test_support::remove_session(&agent);
    printer.finish(result).unwrap();
    assert_eq!(checkpoint, None);
    assert_eq!(String::from_utf8(out).unwrap(), "waiting\n\nseen it\n");
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains("background task 1 finished\nfinished"),
        "{err}"
    );
}

#[tokio::test]
async fn uses_github_high_contrast_colours() {
    for (dark, keyword) in [(true, "255;148;146"), (false, "160;17;31")] {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                json!({ "command": "python3 - <<'EOF'\ndef f():\n    pass\nEOF" }),
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
            json!({ "command": "echo \"$HOME\" | wc -c" }),
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
        tool_reply("call-1", "bash", json!({ "command": "echo hi" })),
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

#[tokio::test]
async fn writes_piped_answers_byte_for_byte() {
    let answer = format!("done{ESCAPES}\r\n");
    let api = MockApi::start(vec![text_reply(&answer)]).await;
    let mut out = Vec::new();
    let mut printer = Printer::new(&mut out, false, io::sink(), false, false, None);
    let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
    let finished = printer.finish(result);
    finished.unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("{answer}\n"));
}

#[tokio::test]
async fn ends_an_answer_that_broke_off_before_the_error_is_shown() {
    let api = MockApi::start(vec![
        Reply::Events(vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial" } }),
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
        ]),
        Reply::BadRequest("bad".into()),
    ])
    .await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    let mut outputs = Vec::new();
    for _ in 0..2 {
        let mut out = Vec::new();
        let mut printer = Printer::new(&mut out, false, io::sink(), false, false, None);
        let result = agent.prompt("go", &[], |event| printer.event(event)).await;
        let finished = printer.finish(result);
        outputs.push((String::from_utf8(out).unwrap(), finished.is_err()));
    }
    test_support::remove_session(&agent);

    // The broken-off answer ends its line, so the error starts on its
    // own; a run that printed nothing adds no blank line.
    assert_eq!(
        outputs,
        [("partial\n".to_string(), true), (String::new(), true)]
    );
}

#[tokio::test]
async fn prints_only_the_tasks_and_why_one_failed() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            json!({
                "mode": "sequential",
                "tasks": [
                    { "description": "say hi", "prompt": "say hi" },
                    { "description": "say bye", "prompt": "say bye" },
                ],
            }),
        ),
        tool_reply("call-2", "bash", json!({ "command": "true" })),
        text_reply("hi"),
        Reply::BadRequest("bad".into()),
        text_reply("done"),
    ])
    .await;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
    let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
    let finished = printer.finish(result);
    finished.unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "done\n");
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.starts_with("agent sequential, 2 tasks\n\n[agent 1] say hi · 1 tool call · "),
        "{err}"
    );
    assert!(
        err.contains("\n\n[agent 2] say bye\n  This task failed: 400 Bad Request: "),
        "{err}"
    );
}
