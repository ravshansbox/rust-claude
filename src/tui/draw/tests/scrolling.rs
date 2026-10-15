use crate::agent::AgentEvent;
use crate::tui::{
    Activity, Role, UiEvent,
    draw::held_scroll_from_bottom,
    keys::handle_input,
    replay::handle_agent_event,
    test_support::{app_with_reply, press, screen, screen_of_width},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};

#[test]
fn keeps_scrolled_view_still_while_content_grows() {
    assert_eq!(held_scroll_from_bottom(0, 10, 15), 0);
    assert_eq!(held_scroll_from_bottom(1, 15, 20), 6);
    assert_eq!(held_scroll_from_bottom(6, 20, 18), 4);
    assert_eq!(held_scroll_from_bottom(1, 15, 10), 0);
}

#[test]
fn keeps_the_spinner_and_queued_prompts_visible_when_scrolled_up() {
    let mut app = app_with_reply();
    for number in 0..100 {
        app.push(Role::Event, format!("event {number}"));
    }
    app.start(Activity::Working);
    handle_input(Event::Paste("later".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    screen(&mut app);
    press(&mut app, KeyCode::Home);
    let shown = screen(&mut app);
    assert!(shown.contains("Earlier reply"), "{shown}");
    let rows: Vec<&str> = shown.lines().map(str::trim_end).collect();
    let spinner = rows
        .iter()
        .position(|row| row.contains(" working"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert_eq!(rows[spinner + 1], "queued: later", "{shown}");
    assert!(rows[spinner + 2].starts_with('─'), "{shown}");
    press(&mut app, KeyCode::End);
    let shown = screen(&mut app);
    assert_eq!(shown.matches(" working").count(), 1, "{shown}");
    assert!(shown.contains("event 99"), "{shown}");
}

#[test]
fn shows_the_spinner_once_when_scrolled_up_a_little() {
    let mut app = app_with_reply();
    for number in 0..100 {
        app.push(Role::Event, format!("event {number}"));
    }
    app.start(Activity::Working);
    handle_input(Event::Paste("later".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    screen(&mut app);
    for key in [KeyCode::Up, KeyCode::Down, KeyCode::Down, KeyCode::Down] {
        press(&mut app, key);
        let shown = screen(&mut app);
        assert_eq!(shown.matches(" working").count(), 1, "{shown}");
        assert_eq!(shown.matches("queued: later").count(), 1, "{shown}");
    }
    press(&mut app, KeyCode::Up);
    let shown = screen(&mut app);
    assert!(shown.contains("↓ Bottom (End)"), "{shown}");
    assert!(!shown.contains("event 99"), "{shown}");
}

#[test]
fn stays_at_the_bottom_when_only_the_spinner_was_below_the_view() {
    let mut app = app_with_reply();
    app.start(Activity::Working);
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    let mut number = 0;
    while app.max_scroll < 1 {
        app.push(Role::Event, format!("event {number}"));
        number += 1;
        screen(&mut app);
    }
    press(&mut app, KeyCode::Up);
    let shown = screen(&mut app);
    assert!(!shown.contains("↓ Bottom (End)"), "{shown}");
    assert_eq!(app.scroll_from_bottom, 0);
}

#[test]
fn shows_a_scroll_to_bottom_button_above_the_prompt_when_scrolled_up() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    for working in [false, true] {
        let mut app = app_with_reply();
        for number in 0..100 {
            app.push(Role::Event, format!("event {number}"));
        }
        if working {
            app.start(Activity::Working);
        }
        let shown = screen(&mut app);
        assert!(!shown.contains("↓ Bottom (End)"), "{shown}");
        press(&mut app, KeyCode::Home);
        let shown = screen(&mut app);
        let rows: Vec<&str> = shown.lines().collect();
        let row = rows
            .iter()
            .position(|row| row.ends_with("↓ Bottom (End)"))
            .unwrap_or_else(|| panic!("{shown}"));
        assert!(rows[row + 1].starts_with('─'), "{shown}");
        assert_eq!(rows[row].contains(" working"), working, "{shown}");
        let column = 100 - u16::try_from("↓ Bottom (End)".chars().count()).unwrap();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            let event = Event::Mouse(MouseEvent {
                kind,
                column,
                row: u16::try_from(row).unwrap(),
                modifiers: KeyModifiers::NONE,
            });
            handle_input(event, &mut app, |_| {});
        }
        let shown = screen(&mut app);
        assert!(shown.contains("event 99"), "{shown}");
        assert!(!shown.contains("↓ Bottom (End)"), "{shown}");
        assert!(app.input.is_empty(), "{:?}", app.input);
    }
}

#[test]
fn scrolls_back_a_page_and_returns_with_the_mouse_wheel() {
    use crossterm::event::{MouseEvent, MouseEventKind};
    let mut app = app_with_reply();
    for number in 0..100 {
        app.push(Role::Event, format!("event {number}"));
    }
    screen(&mut app);
    press(&mut app, KeyCode::PageUp);
    let shown = screen(&mut app);
    assert!(!shown.contains("event 99"), "{shown}");
    assert!(shown.contains("↓ Bottom (End)"), "{shown}");
    for _ in 0..100 {
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        handle_input(event, &mut app, |_| {});
    }
    let shown = screen(&mut app);
    assert!(shown.contains("event 99"), "{shown}");
    assert!(!shown.contains("↓ Bottom (End)"), "{shown}");
}

#[test]
fn shows_the_end_of_a_conversation_longer_than_65535_lines() {
    let mut app = app_with_reply();
    for number in 0..70_000 {
        app.push(Role::Event, format!("event {number}"));
    }
    let shown = screen(&mut app);
    assert!(shown.contains("event 69999"), "{shown}");
    press(&mut app, KeyCode::Home);
    let shown = screen(&mut app);
    assert!(shown.contains("Earlier reply"), "{shown}");
    press(&mut app, KeyCode::PageDown);
    let shown = screen(&mut app);
    assert!(shown.contains("event 20"), "{shown}");
}

#[test]
fn follows_the_end_after_resizing_and_while_a_reply_grows() {
    let mut app = app_with_reply();
    let words: String = (0..100).map(|number| format!("word{number:02} ")).collect();
    app.push(Role::Event, format!("{words}END"));
    let shown = screen_of_width(&mut app, 100);
    assert!(shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains("END"), "{shown}");
    let shown = screen_of_width(&mut app, 30);
    assert!(!shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains("END"), "{shown}");
    press(&mut app, KeyCode::Home);
    let shown = screen_of_width(&mut app, 30);
    assert!(shown.contains("Earlier reply"), "{shown}");
    press(&mut app, KeyCode::End);
    handle_agent_event(UiEvent::Agent(AgentEvent::Text("Start".into())), &mut app);
    let shown = screen_of_width(&mut app, 30);
    assert!(shown.contains("Start"), "{shown}");
    let more: String = (0..30).map(|number| format!("\n\nline {number}")).collect();
    handle_agent_event(UiEvent::Agent(AgentEvent::Text(more)), &mut app);
    let shown = screen_of_width(&mut app, 30);
    assert!(shown.contains("line 29"), "{shown}");
    assert!(!shown.contains("Start"), "{shown}");
}

#[test]
fn shows_the_first_row_after_one_press_of_home() {
    let mut app = app_with_reply();
    for number in 0..100 {
        app.push(Role::Event, format!("event {number}"));
    }
    screen(&mut app);
    press(&mut app, KeyCode::Home);
    let shown = screen(&mut app);
    assert!(
        shown
            .lines()
            .next()
            .unwrap()
            .starts_with("Rust Coding Agent v"),
        "{shown}"
    );
}
