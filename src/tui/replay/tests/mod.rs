mod background;
mod notifications;
mod tasks;
mod tool_calls;
mod update;
mod web;
mod workspace;

use super::{handle_agent_event, replay_messages};
use crate::agent::{self, AgentEvent};
use crate::images;
use crate::skills::{Scope, Skill};
use crate::tools;
use crate::tui::{
    Activity, App, Role, UiEvent,
    keys::{Action, handle_input},
    test_support::new_app,
};
use crossterm::event::{Event, KeyCode, KeyEvent};
use serde_json::json;
use std::io;

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
