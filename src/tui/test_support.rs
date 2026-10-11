use super::keys::{Action, handle_input};
use super::{App, Role, draw::draw};
use crate::agent::Stats;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Cell};

pub(super) fn new_app() -> App {
    App::new("model", "medium", Stats::default())
}

pub(super) fn press(app: &mut App, code: KeyCode) -> String {
    press_with(app, code, KeyModifiers::NONE, |_| {});
    app.input.clone()
}

pub(super) fn press_with(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    act: impl FnMut(Action),
) -> bool {
    handle_input(Event::Key(KeyEvent::new(code, modifiers)), app, act)
}

pub(super) fn screen(app: &mut App) -> String {
    screen_of_width(app, 100)
}

pub(super) fn screen_of_width(app: &mut App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn app_with_reply() -> App {
    let mut app = new_app();
    app.push(Role::Assistant, "Earlier reply");
    app
}
