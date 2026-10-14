use super::super::{Action, handle_input};
use crate::session::SessionSummary;
use crate::tui::{
    Activity, UiEvent, handle_agent_event,
    test_support::{app_with_reply, new_app, press, press_with, screen},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use std::time::{Duration, SystemTime};

#[test]
fn resumes_the_session_picked_from_the_list() {
    let mut app = new_app();
    let mut listed = false;
    handle_input(Event::Paste("/resume".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        listed |= matches!(action, Action::ListSessions);
    });
    assert!(listed);
    let sessions = [("0", 125), ("1", 3 * 3_600 + 10), ("2", 2 * 86_400 + 10)]
        .map(|(id, age)| SessionSummary {
            id: id.into(),
            modified: SystemTime::now() - Duration::from_secs(age),
            preview: format!("prompt {id}\nmore"),
        })
        .into();
    handle_agent_event(UiEvent::Sessions(Ok(sessions)), &mut app);
    let labels: Vec<&str> = app
        .picker
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|(_, label)| label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "  2m ago  prompt 0",
            "  3h ago  prompt 1",
            "  2d ago  prompt 2"
        ]
    );
    press(&mut app, KeyCode::Down);
    let mut resumed = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Resume(id) = action {
            resumed = Some(id);
        }
    });
    assert_eq!(resumed.as_deref(), Some("1"));
    assert!(app.picker.is_none());
    assert_eq!(app.activity, Some(Activity::Resuming));
}

#[test]
fn resumes_behind_a_background_turn_started_while_picking() {
    let mut app = new_app();
    app.start(Activity::LoadingSessions);
    let sessions = vec![SessionSummary {
        id: "0".into(),
        modified: SystemTime::now(),
        preview: "prompt".into(),
    }];
    handle_agent_event(UiEvent::Sessions(Ok(sessions)), &mut app);
    let message = crate::tools::background::message(1, "echo hi", "hi\n");
    handle_agent_event(UiEvent::Background(message), &mut app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.activity, Some(Activity::Working));
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.activity, Some(Activity::Resuming));
}

#[test]
fn says_when_there_is_no_session_to_resume() {
    let mut app = new_app();
    app.start(Activity::LoadingSessions);
    handle_agent_event(UiEvent::Sessions(Ok(Vec::new())), &mut app);
    assert!(app.picker.is_none());
    assert!(!app.busy());
    assert_eq!(app.messages.last().unwrap().text, "no session to resume");
}

#[test]
fn ignores_escape_while_loading_sessions() {
    let mut app = new_app();
    app.start(Activity::LoadingSessions);
    let mut cancelled = false;
    let quit = press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |action| {
        cancelled |= matches!(action, Action::Cancel);
    });
    assert!(!quit);
    assert!(!cancelled);
    assert_eq!(app.activity, Some(Activity::LoadingSessions));
}

#[test]
fn starts_a_new_session_without_the_earlier_conversation() {
    let mut app = app_with_reply();
    let mut started = false;
    handle_input(Event::Paste("/new".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        started |= matches!(action, Action::NewSession);
    });
    assert!(started);
    assert!(screen(&mut app).contains("starting new session"));
    handle_agent_event(UiEvent::NewSession(Ok(())), &mut app);
    let shown = screen(&mut app);
    assert!(!shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains("new session"), "{shown}");
    assert!(!shown.contains("starting new session"), "{shown}");
}
