use super::super::Printer;
use super::without_colours;
use crate::agent::test_support::{self, MockApi, text_reply, tool_reply};
use crate::tools::test_support::TemporaryDir;
use serde_json::json;
use std::io;

const ESCAPES: &str = "\x1b]52;c;aGk=\x07\x1b[2J\u{9b}2J\r";

fn has_controls(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
}

#[tokio::test]
async fn keeps_escape_sequences_from_the_model_and_tools_out_of_the_terminal() {
    let directory = TemporaryDir::new("print-escape");
    let path = directory.path().join("escape");
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "write",
            &json!({ "path": path, "content": format!("one{ESCAPES}\ntwo\n") }),
        ),
        tool_reply(
            "call-2",
            "bash",
            &json!({
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
