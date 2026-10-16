use crate::session::SessionSummary;
use crate::skills::{Scope, Skill};
use crate::tui::{
    App, Role, UiEvent,
    keys::handle_input,
    replay::handle_agent_event,
    test_support::{app_with_reply, press, screen},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

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
    for _ in 0..37 {
        press(&mut app, KeyCode::Down);
    }
    let shown = screen(&mut app);
    assert!(shown.contains("/skill:skill29"), "{shown}");
    assert!(shown.contains("/skill:skill20"), "{shown}");
    assert!(!shown.contains("/skill:skill19"), "{shown}");
}

#[test]
fn shades_the_command_list_like_tool_calls() {
    let mut app = app_with_reply();
    app.push(Role::Tool, "! make");
    handle_input(Event::Paste("/".into()), &mut app, |_| {});
    let shown = screen(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| super::super::draw(frame, &mut app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let cell_of = |text: &str| {
        let (y, row) = shown
            .lines()
            .enumerate()
            .find(|(_, row)| row.contains(text))
            .unwrap_or_else(|| panic!("{shown}"));
        let x = row.find(text).unwrap();
        (u16::try_from(x).unwrap(), u16::try_from(y).unwrap())
    };
    assert_eq!(buffer[cell_of("/compact")].bg, buffer[cell_of("make")].bg);
}
