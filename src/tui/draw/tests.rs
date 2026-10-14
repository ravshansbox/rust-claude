use super::held_scroll_from_bottom;
use crate::agent::AgentEvent;
use crate::session::SessionSummary;
use crate::skills::{Scope, Skill};
use crate::tui::{
    Activity, App, Role, UiEvent, handle_agent_event, handle_input,
    test_support::{app_with_reply, new_app, press, screen, screen_of_width},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn shows_the_input_without_a_marker() {
    let mut app = app_with_reply();
    handle_input(Event::Paste("lorem ipsum dolor".into()), &mut app, |_| {});
    let shown = screen_of_width(&mut app, 10);
    let rows: Vec<&str> = shown.lines().collect();
    let first = rows
        .iter()
        .position(|row| row.starts_with("lorem"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert_eq!(rows[first], "lorem ipsu", "{shown}");
    assert_eq!(rows[first + 1], "m dolor   ", "{shown}");
    let mut terminal = Terminal::new(TestBackend::new(10, 24)).unwrap();
    terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
    let cursor = terminal.get_cursor_position().unwrap();
    assert_eq!((cursor.x, cursor.y), (7, u16::try_from(first).unwrap() + 1));
    press(&mut app, KeyCode::Up);
    assert_eq!(app.cursor, "lorem i".len());
}

fn rows_from<'a>(shown: &'a str, start: &str) -> Vec<&'a str> {
    let rows: Vec<&str> = shown.lines().map(str::trim_end).collect();
    let first = rows
        .iter()
        .position(|row| row.starts_with(start))
        .unwrap_or_else(|| panic!("{shown}"));
    rows[first..].to_vec()
}

#[test]
fn shows_notices_in_a_row_without_blank_rows_between_them() {
    let mut app = new_app();
    app.push(Role::Event, "first notice");
    app.push(Role::Event, "second notice");
    app.push(Role::Assistant, "reply");
    app.push(Role::Event, "last notice");
    let shown = screen(&mut app);
    let rows: Vec<&str> = shown.lines().map(str::trim_end).collect();
    assert!(rows[0].starts_with("Rust Coding Agent v"), "{shown}");
    assert_eq!(
        rows[1..7],
        [
            "first notice",
            "second notice",
            "",
            "reply",
            "",
            "last notice"
        ],
        "{shown}"
    );
}

#[test]
fn shows_a_reply_at_the_left_edge() {
    let mut app = new_app();
    app.push(Role::Assistant, "alpha beta gamma delta\n\nsecond part");
    let shown = screen_of_width(&mut app, 14);
    let rows = rows_from(&shown, "alpha");
    assert_eq!(
        rows[..4],
        ["alpha beta", "gamma delta", "", "second part"],
        "{shown}"
    );
}

#[test]
fn shows_the_same_rows_from_any_scroll_position() {
    use ratatui::{
        buffer::{Buffer, Cell},
        layout::Rect,
        widgets::Widget,
    };
    let mut app = new_app();
    let words: String = (0..30).map(|number| format!("word{number:02} ")).collect();
    app.push(Role::Event, "first event");
    app.push(Role::Assistant, format!("{words}\n\n{words}\n\nend"));
    app.push(Role::Event, format!("{words}last"));
    let width = 20;
    screen_of_width(&mut app, width);
    let rows = |top: usize, height: u16| {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        super::conversation(&app, width, top, height).render(area, &mut buffer);
        buffer
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
    };
    let all = rows(0, 200);
    let total = all.iter().rposition(|row| !row.trim().is_empty()).unwrap() + 1;
    assert!(total > 35, "{all:?}");
    for top in 0..total {
        let end = (top + 5).min(all.len());
        assert_eq!(rows(top, 5)[..end - top], all[top..end], "from row {top}");
    }
}

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

fn open_sessions(app: &mut App, count: usize) {
    let sessions = (0..count)
        .map(|index| SessionSummary {
            id: index.to_string(),
            modified: std::time::SystemTime::now(),
            preview: format!("prompt {index:02}"),
        })
        .collect();
    handle_agent_event(UiEvent::Sessions(Ok(sessions)), app);
}

#[test]
fn shows_at_most_ten_items_with_a_scrollbar() {
    let mut app = app_with_reply();
    open_sessions(&mut app, 15);
    let shown = screen(&mut app);
    assert!(shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains("prompt 00"), "{shown}");
    assert!(shown.contains("prompt 09"), "{shown}");
    assert!(!shown.contains("prompt 10"), "{shown}");
    assert!(shown.contains('█'), "{shown}");
    for _ in 0..12 {
        press(&mut app, KeyCode::Down);
    }
    let shown = screen(&mut app);
    assert!(!shown.contains("prompt 02"), "{shown}");
    assert!(shown.contains("prompt 03"), "{shown}");
    assert!(shown.contains("prompt 12"), "{shown}");
    assert!(!shown.contains("prompt 13"), "{shown}");
}

#[test]
fn hides_the_scrollbar_when_items_fit() {
    let mut app = app_with_reply();
    open_sessions(&mut app, 10);
    let shown = screen(&mut app);
    assert!(shown.contains("prompt 09"), "{shown}");
    assert!(!shown.contains('█'), "{shown}");
}

#[test]
fn shows_at_most_ten_history_prompts_with_a_scrollbar() {
    let mut app = app_with_reply();
    app.prompt_history = (0..15).map(|index| format!("prompt {index:02}")).collect();
    handle_input(
        Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        &mut app,
        |_| {},
    );
    let shown = screen(&mut app);
    assert!(shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains(" Folder "), "{shown}");
    assert!(shown.contains("prompt 14"), "{shown}");
    assert!(shown.contains("prompt 05"), "{shown}");
    assert!(!shown.contains("prompt 04"), "{shown}");
    assert!(shown.contains('█'), "{shown}");
}

#[test]
fn keeps_the_selected_command_visible_in_a_long_list() {
    let mut app = app_with_reply();
    app.skills = (0..30)
        .map(|index| Skill {
            name: format!("skill{index:02}"),
            description: format!("skill number {index}"),
            path: "/skills/SKILL.md".into(),
            base_dir: "/skills".into(),
            disable_model_invocation: false,
            scope: Scope::Global,
        })
        .collect();
    handle_input(Event::Paste("/".into()), &mut app, |_| {});
    let shown = screen(&mut app);
    assert!(shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains("/compact"), "{shown}");
    assert!(!shown.contains("/skill:skill03"), "{shown}");
    assert!(shown.contains('█'), "{shown}");
    for _ in 0..36 {
        press(&mut app, KeyCode::Down);
    }
    let shown = screen(&mut app);
    assert!(shown.contains("/skill:skill29"), "{shown}");
    assert!(shown.contains("/skill:skill20"), "{shown}");
    assert!(!shown.contains("/skill:skill19"), "{shown}");
}
