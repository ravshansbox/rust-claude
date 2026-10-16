use super::super::Printer;
use crate::agent::test_support::{self, MockApi, Reply, text_reply, tool_reply};
use serde_json::json;
use std::{io, sync::atomic::AtomicBool};

#[tokio::test]
async fn ends_an_answer_that_broke_off_before_the_error_is_shown() {
    let api = MockApi::start(vec![
        Reply::Events(vec![
            test_support::message_start(),
            test_support::text_block_start(),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial" } }),
            json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
        ]),
        Reply::BadRequest("bad".into()),
    ])
    .await;
    let mut agent = test_support::agent(&api);
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
            &json!({
                "mode": "sequential",
                "tasks": [
                    { "description": "say hi", "prompt": "say hi" },
                    { "description": "say bye", "prompt": "say bye" },
                ],
            }),
        ),
        tool_reply("call-2", "bash", &json!({ "command": "true" })),
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

#[tokio::test]
async fn loops_the_prompt_in_new_sessions_until_a_run_fails() {
    let api = MockApi::start(vec![
        text_reply("one"),
        text_reply("two"),
        Reply::BadRequest("bad".into()),
    ])
    .await;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
    let mut agent = test_support::agent(&api);
    let mut checkpoint = None;
    let stop_after_run = AtomicBool::new(false);
    let result = super::super::run_loop(
        &mut agent,
        &mut printer,
        "go",
        &[],
        &mut checkpoint,
        &stop_after_run,
    )
    .await;
    test_support::remove_session(&agent);
    assert!(printer.finish(result).is_err());
    assert_eq!(String::from_utf8(out).unwrap(), "one\n\ntwo\n");
    let err = String::from_utf8(err).unwrap();
    let rounds: Vec<_> = err
        .lines()
        .filter(|line| line.starts_with("loop "))
        .collect();
    assert_eq!(rounds, ["loop 1", "loop 2", "loop 3"], "{err}");
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert_eq!(request["messages"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn stops_the_loop_after_the_current_run_when_asked() {
    let api = MockApi::start(vec![text_reply("one"), text_reply("two")]).await;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
    let mut agent = test_support::agent(&api);
    let mut checkpoint = None;
    let stop_after_run = AtomicBool::new(true);
    let result = super::super::run_loop(
        &mut agent,
        &mut printer,
        "go",
        &[],
        &mut checkpoint,
        &stop_after_run,
    )
    .await;
    test_support::remove_session(&agent);
    printer.finish(result).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "one\n");
    assert_eq!(api.requests().await.len(), 1);
}
