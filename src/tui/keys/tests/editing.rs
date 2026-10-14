use super::super::{Action, handle_input};
use crate::tui::test_support::{new_app, press, press_with};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[test]
fn edits_emoji_as_whole_graphemes() {
    let mut app = new_app();
    handle_input(Event::Paste("a👍🏽❤️".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Left);
    assert_eq!(app.cursor, "a👍🏽".len());
    press(&mut app, KeyCode::Left);
    assert_eq!(app.cursor, "a".len());
    press(&mut app, KeyCode::Right);
    assert_eq!(app.cursor, "a👍🏽".len());
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.input, "a👍🏽");
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.input, "a");
}

#[test]
fn moves_by_word_with_alt() {
    let mut app = new_app();
    handle_input(Event::Paste("one two".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Left, KeyModifiers::ALT, |_| {});
    assert_eq!(app.cursor, "one ".len());
    press_with(&mut app, KeyCode::Char('b'), KeyModifiers::ALT, |_| {});
    assert_eq!(app.cursor, 0);
    press_with(&mut app, KeyCode::Right, KeyModifiers::ALT, |_| {});
    assert_eq!(app.cursor, "one".len());
    press_with(&mut app, KeyCode::Char('f'), KeyModifiers::ALT, |_| {});
    assert_eq!(app.cursor, "one two".len());
    assert_eq!(app.input, "one two");
}

#[test]
fn deletes_previous_word() {
    let mut app = new_app();
    handle_input(Event::Paste("one two three".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Left);
    press_with(&mut app, KeyCode::Backspace, KeyModifiers::ALT, |_| {});
    assert_eq!(app.input, "one two e");
    press_with(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL, |_| {});
    assert_eq!(app.input, "one e");
    assert_eq!(app.cursor, "one ".len());
}

#[test]
fn edits_input_at_cursor() {
    let mut app = new_app();
    handle_input(Event::Paste("héllo".into()), &mut app, |_| {});
    for code in [KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Left] {
        press(&mut app, code);
    }
    press(&mut app, KeyCode::Backspace);
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Right);
    handle_input(Event::Paste("!".into()), &mut app, |_| {});
    assert_eq!(app.input, "jé!llo");
    assert_eq!(app.cursor, "jé!".len());
    press_with(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL, |_| {});
    assert_eq!(app.cursor, 0);
    press_with(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL, |_| {});
    assert_eq!(app.cursor, app.input.len());
}

#[test]
fn adds_newline_with_shift_or_alt_enter() {
    let mut app = new_app();
    let mut submitted = false;
    for modifiers in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
        handle_input(Event::Paste("a".into()), &mut app, |_| {});
        press_with(&mut app, KeyCode::Enter, modifiers, |_| submitted = true);
    }
    assert!(!submitted);
    assert_eq!(app.input, "a\na\n");
    assert_eq!(app.cursor, app.input.len());
}

#[test]
fn moves_between_input_lines_with_up_and_down() {
    let mut app = new_app();
    app.prompt_history.push("earlier".into());
    handle_input(Event::Paste("one\ntwo".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Up);
    assert_eq!(app.cursor, "one".len());
    press(&mut app, KeyCode::Down);
    assert_eq!(app.cursor, "one\ntwo".len());
    assert_eq!(app.input, "one\ntwo");
    app.input = "abcdefg".into();
    app.cursor = 6;
    app.input_width = 4;
    press(&mut app, KeyCode::Up);
    assert_eq!(app.cursor, 2);
}

#[test]
fn pastes_multiple_lines_without_submitting() {
    let mut app = new_app();
    let mut submitted = false;
    let quit = handle_input(Event::Paste("first\r\nsecond".into()), &mut app, |action| {
        submitted |= matches!(action, Action::Submit(..));
    });
    assert!(!quit);
    assert!(!submitted);
    assert_eq!(app.input, "first\nsecond");
}

#[test]
fn types_each_character_once_when_the_terminal_reports_key_releases() {
    let mut app = new_app();
    let mut release = KeyEvent::from(KeyCode::Char('a'));
    release.kind = KeyEventKind::Release;
    press(&mut app, KeyCode::Char('a'));
    handle_input(Event::Key(release), &mut app, |_| {});
    assert_eq!(app.input, "a");
}
