mod tasks;
mod update;
mod web;
mod workspace;

use super::{handle_agent_event, replay_messages};
use crate::agent::{self, AgentEvent};
use crate::images;
use crate::skills::{Scope, Skill};
use crate::tools;
use crate::tui::{Action, Activity, App, Role, UiEvent, handle_input, test_support::new_app};
use crossterm::event::{Event, KeyCode, KeyEvent};
use serde_json::json;
use std::io;

#[test]
fn notifies_when_a_run_ends_while_the_terminal_is_unfocused() {
    let mut app = new_app();
    handle_input(Event::FocusLost, &mut app, |_| {});
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.notification.take().as_deref(), Some("Done"));
    handle_agent_event(
        UiEvent::Done(Err(anyhow::anyhow!("overloaded\nretry later"))),
        &mut app,
    );
    assert_eq!(
        app.notification.take().as_deref(),
        Some("Failed: overloaded")
    );
}

#[test]
fn does_not_notify_when_focused_cancelled_or_turned_off() {
    let mut focused = new_app();
    handle_input(Event::FocusLost, &mut focused, |_| {});
    handle_input(Event::FocusGained, &mut focused, |_| {});
    let mut cancelled = new_app();
    handle_input(Event::FocusLost, &mut cancelled, |_| {});
    cancelled.start(Activity::Cancelling);
    let mut off = new_app();
    off.notifications = false;
    handle_input(Event::FocusLost, &mut off, |_| {});
    for mut app in [focused, cancelled, off] {
        handle_agent_event(UiEvent::Done(Ok(())), &mut app);
        assert_eq!(app.notification, None);
    }
}

#[test]
fn shows_queued_prompt_when_the_agent_adds_it() {
    let mut app = new_app();
    handle_agent_event(UiEvent::Agent(AgentEvent::Queued("next".into())), &mut app);
    let last = app.messages.last().unwrap();
    assert!(matches!(last.role, Role::User));
    assert_eq!(last.text, "next");
}

#[test]
fn shows_a_finished_background_command_while_working_on_it() {
    let message = tools::background::message(1, "echo hi", "hi\n");
    let mut app = new_app();
    handle_agent_event(UiEvent::Background(message.clone()), &mut app);
    assert!(app.busy());
    assert!(matches!(app.messages.last().unwrap().role, Role::Tool));
    assert_eq!(
        app.messages.last().unwrap().text,
        "background task 1 finished\nhi"
    );
    let mut resumed = new_app();
    replay_messages(
        &mut resumed,
        &[json!({ "role": "user", "content": message })],
    );
    assert_eq!(
        resumed.messages.last().unwrap().text,
        "background task 1 finished\nhi"
    );
    assert_eq!(resumed.prompt_history, Vec::<String>::new());
}

#[test]
fn stays_busy_with_a_prompt_sent_as_a_background_turn_starts() {
    let mut app = new_app();
    let mut sent = false;
    handle_input(Event::Paste("hello".into()), &mut app, |_| {});
    handle_input(
        Event::Key(KeyEvent::from(KeyCode::Enter)),
        &mut app,
        |action| sent |= matches!(action, Action::Submit(..)),
    );
    let message = tools::background::message(1, "echo hi", "hi\n");
    handle_agent_event(UiEvent::Background(message), &mut app);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert!(sent);
    assert_eq!(app.activity, Some(Activity::Working));
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert!(!app.busy());
}

#[test]
fn replays_shell_commands() {
    let mut app = new_app();
    replay_messages(
        &mut app,
        &[json!({ "role": "user", "content": agent::shell_message("pwd", "/tmp\n") })],
    );
    assert!(matches!(app.messages.last().unwrap().role, Role::Tool));
    assert_eq!(app.messages.last().unwrap().text, "! pwd\n/tmp");
    assert_eq!(app.prompt_history, ["!pwd"]);
}

#[test]
fn replays_failed_tool_calls() {
    let mut app = new_app();
    let read = |id: &str, path: &str| json!({ "type": "tool_use", "id": id, "name": "read", "input": { "path": path } });
    let result = |id: &str, is_error: bool| json!({ "type": "tool_result", "tool_use_id": id, "content": "missing", "is_error": is_error });
    replay_messages(
        &mut app,
        &[
            json!({ "role": "assistant", "content": [read("1", "a.rs"), read("2", "b.rs"), read("3", "c.rs")] }),
            json!({ "role": "user", "content": [result("1", false), result("2", true), result("3", false)] }),
        ],
    );
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["read a.rs, b.rs", "read failed: missing", "read c.rs"]
    );
}

#[test]
fn shows_a_failed_tool_call_live_as_it_is_replayed() {
    let mut app = new_app();
    for (path, error) in [("a.rs", None), ("b.rs", Some("missing")), ("c.rs", None)] {
        let input = json!({ "path": path });
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolStart {
                name: "read".into(),
                summary: tools::summary("read", &input),
                diff: None,
            }),
            &mut app,
        );
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolDone {
                name: "read".into(),
                error: error.map(Into::into),
                note: None,
            }),
            &mut app,
        );
    }
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["read a.rs, b.rs", "read failed: missing", "read c.rs"]
    );
}

#[test]
fn shows_streamed_thinking_live_as_it_is_replayed() {
    let mut live = new_app();
    for event in [
        AgentEvent::Thinking("Let me ".into()),
        AgentEvent::Thinking("check.".into()),
        AgentEvent::Text("Done.".into()),
    ] {
        handle_agent_event(UiEvent::Agent(event), &mut live);
    }
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[json!({ "role": "assistant", "content": [
            { "type": "thinking", "thinking": "Let me check.", "signature": "s" },
            { "type": "text", "text": "Done." },
        ] })],
    );
    let messages = |app: &App| {
        app.messages
            .iter()
            .map(|message| (matches!(message.role, Role::Thinking), message.text.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        messages(&live)[1..],
        [(true, "Let me check.".into()), (false, "Done.".into())]
    );
    assert_eq!(messages(&live), messages(&replayed));
}

#[test]
fn replays_a_cancelled_tool_call_as_it_appeared_live() {
    let mut app = new_app();
    let read = |id: &str, path: &str| json!({ "type": "tool_use", "id": id, "name": "read", "input": { "path": path } });
    let result = |id: &str, content: &str, is_error: bool| json!({ "type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error });
    replay_messages(
        &mut app,
        &[
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [read("1", "a.rs"), read("2", "b.rs"), read("3", "c.rs")] }),
            json!({ "role": "user", "content": [result("1", "a", false), result("2", "cancelled by the user", true), result("3", "cancelled by the user", true)] }),
            json!({ "role": "assistant", "stop_reason": "aborted", "content": [] }),
        ],
    );
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(texts, ["hello", "read a.rs, b.rs", "cancelled"]);
}

#[test]
fn replays_compaction_as_event() {
    let mut app = new_app();
    replay_messages(
        &mut app,
        &[
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "stop_reason": "compacted", "content": [], "summary": "greeted" }),
        ],
    );
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(texts, ["hello", "compacted conversation"]);
}

#[test]
fn shows_skill_commands_live_and_replayed() {
    let mut live = new_app();
    live.skills = vec![Skill {
        name: "demo".into(),
        description: "Run\nthe demo.".into(),
        path: "/skills/demo/SKILL.md".into(),
        base_dir: "/skills/demo".into(),
        disable_model_invocation: false,
        scope: Scope::Global,
    }];
    handle_input(Event::Paste("/sk".into()), &mut live, |_| {});
    assert_eq!(
        live.visible_suggestions().unwrap().items,
        [(
            "/skill:demo".to_string(),
            "[global] Run the demo.".to_string()
        )]
    );
    live.input = "/skill:demo fix it".into();
    let mut submitted = None;
    handle_input(
        Event::Key(KeyEvent::from(KeyCode::Enter)),
        &mut live,
        |action| {
            if let Action::Submit(prompt, _, _) = action {
                submitted = Some(prompt);
            }
        },
    );
    assert_eq!(submitted.as_deref(), Some("/skill:demo fix it"));
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[
            json!({ "role": "user", "content": "<skill name=\"demo\" location=\"/skills/demo/SKILL.md\">\nReferences are relative to /skills/demo.\n\nBody\n</skill>\n\nfix it" }),
        ],
    );
    let texts = |app: &App| -> Vec<String> {
        app.messages[1..]
            .iter()
            .map(|message| message.text.clone())
            .collect()
    };
    assert_eq!(texts(&live), ["[skill] demo", "fix it"]);
    assert_eq!(texts(&replayed), texts(&live));
    assert_eq!(replayed.prompt_history, live.prompt_history);
}

#[test]
fn shows_replacement_count_live_and_replayed() {
    let input = json!({ "path": "a.rs", "old_text": "a", "new_text": "b", "replace_all": true });
    let mut live = new_app();
    handle_agent_event(
        UiEvent::Agent(AgentEvent::ToolStart {
            name: "edit".into(),
            summary: tools::summary("edit", &input),
            diff: tools::diff("edit", &input),
        }),
        &mut live,
    );
    handle_agent_event(
        UiEvent::Agent(AgentEvent::ToolDone {
            name: "edit".into(),
            error: None,
            note: Some("3 replacements".into()),
        }),
        &mut live,
    );
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "1", "name": "edit", "input": input }] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "1", "content": "edited a.rs (3 replacements)", "is_error": false }] }),
        ],
    );
    let texts = |app: &App| -> Vec<String> {
        app.messages[1..]
            .iter()
            .map(|message| message.text.clone())
            .collect()
    };
    assert_eq!(texts(&live), ["edit a.rs\n-a\n+b", "edit: 3 replacements"]);
    assert_eq!(texts(&replayed), texts(&live));
}

#[test]
fn replaces_the_conversation_with_the_resumed_session() {
    let mut app = new_app();
    app.push(Role::User, "old question");
    app.push(Role::Assistant, "old answer");
    handle_agent_event(
        UiEvent::Resumed(Ok(vec![
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi there" }] }),
        ])),
        &mut app,
    );
    let texts: Vec<_> = app.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["hello", "hi there", "resumed session"]);
}

#[test]
fn shows_why_resuming_a_session_failed() {
    let mut app = new_app();
    let error = anyhow::Error::new(io::Error::from(io::ErrorKind::PermissionDenied))
        .context("reading /sessions/x.jsonl");
    handle_agent_event(UiEvent::Resumed(Err(error)), &mut app);
    assert_eq!(
        app.messages.last().unwrap().text,
        "error: reading /sessions/x.jsonl: permission denied"
    );
}

#[test]
fn shows_why_pasting_an_image_failed() {
    let mut app = new_app();
    let error = images::prepare(b"not an image".to_vec()).unwrap_err();
    handle_agent_event(UiEvent::ImagePasted(Err(error)), &mut app);
    let text = &app.messages.last().unwrap().text;
    assert!(
        text.starts_with("failed to paste image: unsupported image format: "),
        "{text}"
    );
}

fn texts(app: &App) -> Vec<String> {
    app.messages[1..]
        .iter()
        .map(|message| message.text.clone())
        .collect()
}
