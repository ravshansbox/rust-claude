use super::super::{Action, handle_input};
use crate::tui::test_support::{app_with_reply, new_app, press, press_with, screen};
use crossterm::event::{Event, KeyCode, KeyModifiers};

#[test]
fn offers_off_only_for_models_that_can_turn_thinking_off() {
    let mut app = new_app();
    app.model = "claude-haiku-5-5".into();
    app.effort = "max";
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.effort, "off");
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.effort, "low");
    handle_input(Event::Paste("/effort".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    let items: Vec<&str> = app
        .picker
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|(value, _)| value.as_str())
        .collect();
    assert_eq!(items, ["off", "low", "medium", "high", "xhigh", "max"]);

    let mut app = new_app();
    app.model = "claude-opus-5-5".into();
    app.effort = "max";
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.effort, "low");
    handle_input(Event::Paste("/effort off".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.effort, "low");
    assert_eq!(
        app.messages.last().unwrap().text,
        "unknown effort level: off (options: low, medium, high, xhigh, max)"
    );
}

#[test]
fn switches_off_to_the_default_on_a_model_that_cannot_turn_thinking_off() {
    let mut app = new_app();
    app.model = "claude-sonnet-5-5".into();
    app.effort = "off";
    press_with(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL, |_| {});
    assert_eq!(app.model, "claude-haiku-5-5");
    assert_eq!(app.effort, "off");
    press_with(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL, |_| {});
    assert_eq!(app.model, "claude-fable-5-1");
    assert_eq!(app.effort, "medium");
    assert_eq!(app.messages.last().unwrap().text, "model: fable");
}

#[test]
fn reports_the_chosen_effort_level() {
    let mut app = new_app();
    handle_input(Event::Paste("/effort high".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.effort, "high");
    assert_eq!(app.messages.last().unwrap().text, "effort: high");
}

#[test]
fn sets_the_effort_level_picked_from_the_list() {
    let mut app = new_app();
    handle_input(Event::Paste("/effort".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    let picker = app.picker.as_ref().unwrap();
    assert_eq!(picker.items[picker.selected].0, "medium");
    press(&mut app, KeyCode::Down);
    let mut acted = false;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
        acted = true;
    });
    assert!(!acted);
    assert!(app.picker.is_none());
    assert_eq!(app.effort, "high");
    assert_eq!(app.messages.last().unwrap().text, "effort: high");
}

#[test]
fn compacts_at_the_chosen_effort_level() {
    let mut app = app_with_reply();
    let mut effort = None;
    handle_input(Event::Paste("/compact".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Compact(level) = action {
            effort = Some(level);
        }
    });
    assert_eq!(effort, Some("medium"));
    assert!(screen(&mut app).contains("compacting"));
}
