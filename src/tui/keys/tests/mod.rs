use super::{Action, handle_input, may_change_screen};
use crate::images::Image;
use crate::tui::{
    Activity, App, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, press, press_with, screen},
};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

mod editing;
mod effort;
mod looping;
mod models;
mod queue;
mod quitting;
mod sessions;

#[test]
fn redraws_only_for_events_that_may_change_the_screen() {
    let mouse = |kind| {
        Event::Mouse(MouseEvent {
            kind,
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    };
    let mut release = KeyEvent::from(KeyCode::Char('a'));
    release.kind = KeyEventKind::Release;
    for (event, redraw) in [
        (mouse(MouseEventKind::Moved), false),
        (mouse(MouseEventKind::Drag(MouseButton::Right)), false),
        (mouse(MouseEventKind::Down(MouseButton::Left)), true),
        (mouse(MouseEventKind::Drag(MouseButton::Left)), true),
        (mouse(MouseEventKind::Up(MouseButton::Left)), true),
        (mouse(MouseEventKind::ScrollUp), true),
        (mouse(MouseEventKind::ScrollDown), true),
        (Event::Key(release), false),
        (Event::Key(KeyEvent::from(KeyCode::Char('a'))), true),
        (Event::FocusLost, false),
        (Event::Paste("a".into()), true),
        (Event::Resize(80, 24), true),
    ] {
        assert_eq!(may_change_screen(&event), redraw, "{event:?}");
    }
}

#[test]
fn keeps_typed_text_after_an_unknown_command_or_effort_level() {
    for (typed, error) in [
        (
            "/Users/me/proj/main.rs fails, fix it",
            "unknown command: /Users/me/proj/main.rs",
        ),
        ("/effort loud", "unknown effort level: loud"),
        (
            "/model claude-opus-4-5",
            "unknown model: claude-opus-4-5 (options: claude-fable-5-1, claude-opus-5-5, claude-sonnet-5-5, claude-haiku-5-5)",
        ),
        ("/skill:missing fix it", "unknown skill: missing"),
    ] {
        let mut app = new_app();
        handle_input(Event::Paste(typed.into()), &mut app, |_| {});
        let mut acted = false;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
            acted = true;
        });
        assert!(!acted && !app.busy(), "{typed}");
        assert!(
            app.messages.last().unwrap().text.starts_with(error),
            "{typed}"
        );
        assert_eq!(app.input, typed);
        assert_eq!(app.cursor, typed.len());
        assert_eq!(app.effort, "medium");
    }
}

#[test]
fn stops_the_picker_selection_at_either_end_and_closes_it_with_esc() {
    let mut app = new_app();
    handle_input(Event::Paste("/effort".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    let levels: Vec<String> = app
        .picker
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|(level, _)| level.clone())
        .collect();
    let selected = |app: &App| {
        let picker = app.picker.as_ref().unwrap();
        picker.items[picker.selected].0.clone()
    };
    for _ in 0..=levels.len() {
        press(&mut app, KeyCode::Up);
    }
    assert_eq!(selected(&app), levels[0]);
    for _ in 0..=levels.len() {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(&selected(&app), levels.last().unwrap());
    press(&mut app, KeyCode::Esc);
    assert!(!screen(&mut app).contains("Select effort level"));
    assert_eq!(app.effort, "medium");
}

#[test]
fn runs_shell_commands() {
    let mut app = new_app();
    handle_input(Event::Paste("! ls -a".into()), &mut app, |_| {});
    let mut command = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Shell(text) = action {
            command = Some(text);
        }
    });
    assert_eq!(command.as_deref(), Some("ls -a"));
    assert_eq!(app.activity, Some(Activity::Running));
    let mut cancelled = false;
    press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |action| {
        cancelled |= matches!(action, Action::Cancel);
    });
    assert!(cancelled);
    handle_agent_event(
        UiEvent::Shell("ls -a".into(), Ok("a\nb\n".into())),
        &mut app,
    );
    assert!(!app.busy());
    assert_eq!(app.messages.last().unwrap().text, "! ls -a\na\nb");
}

#[test]
fn ignores_unbound_control_keys() {
    let mut app = new_app();
    app.input = "hello".into();
    for character in ['g', 'd'] {
        let quit = press_with(
            &mut app,
            KeyCode::Char(character),
            KeyModifiers::CONTROL,
            |_| {},
        );
        assert!(!quit);
    }
    assert_eq!(app.input, "hello");
}

#[test]
fn sends_pasted_images_whose_markers_remain() {
    let mut app = new_app();
    let mut pasting = false;
    press_with(
        &mut app,
        KeyCode::Char('v'),
        KeyModifiers::CONTROL,
        |action| pasting = matches!(action, Action::PasteImage),
    );
    assert!(pasting);
    let image = |byte| Image {
        media_type: "image/png",
        data: vec![byte],
    };
    handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(1)))), &mut app);
    handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(2)))), &mut app);
    assert_eq!(app.input, "[image 1][image 2]");
    press(&mut app, KeyCode::Backspace);
    handle_input(Event::Paste(" look".into()), &mut app, |_| {});
    let mut sent = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Submit(prompt, images, _) = action {
            sent = Some((prompt, images));
        }
    });
    assert_eq!(
        sent,
        Some(("[image 1][image 2 look".into(), vec![image(1)]))
    );
    assert_eq!(app.images, Vec::new());
}
