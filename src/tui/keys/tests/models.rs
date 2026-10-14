use super::super::{Action, handle_input};
use crate::tui::test_support::{new_app, press, press_with, screen};
use crossterm::event::{Event, KeyCode, KeyModifiers};

#[test]
fn switches_to_a_listed_model_by_id() {
    let mut app = new_app();
    handle_input(
        Event::Paste("/model claude-haiku-5-5".into()),
        &mut app,
        |_| {},
    );
    let mut chosen = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::SetModel(model) = action {
            chosen = Some(model);
        }
    });
    assert_eq!(chosen.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(app.model, "claude-haiku-5-5");
    assert!(!app.busy());
    assert!(screen(&mut app).contains("model: haiku"));
}

#[test]
fn ctrl_p_switches_between_the_latest_models() {
    let mut app = new_app();
    let mut switched = Vec::new();
    for _ in 0..5 {
        press_with(
            &mut app,
            KeyCode::Char('p'),
            KeyModifiers::CONTROL,
            |action| {
                if let Action::SetModel(model) = action {
                    switched.push(model);
                }
            },
        );
    }
    assert_eq!(
        switched,
        [
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
        ]
    );
    assert_eq!(app.model, "claude-sonnet-5-5");
    let shown = screen(&mut app);
    assert!(shown.contains("model: opus"), "{shown}");
    assert!(shown.contains("sonnet:medium"), "{shown}");
    assert_eq!(app.input, "");
}

#[test]
fn offers_only_the_listed_models_in_the_model_picker() {
    let mut app = new_app();
    let mut acted = false;
    handle_input(Event::Paste("/model".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
        acted = true;
    });
    assert!(!acted && !app.busy());
    let shown = screen(&mut app);
    let rows: Vec<&str> = shown
        .lines()
        .skip_while(|line| !line.starts_with("Select model"))
        .skip(1)
        .take_while(|line| !line.starts_with('─'))
        .map(str::trim)
        .collect();
    assert_eq!(rows, ["fable", "opus", "sonnet", "haiku"], "{shown}");
    press(&mut app, KeyCode::Down);
    let mut chosen = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::SetModel(model) = action {
            chosen = Some(model);
        }
    });
    assert_eq!(chosen.as_deref(), Some("claude-sonnet-5-5"));
}
