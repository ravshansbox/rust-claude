use super::{App, Role, draw::draw, keys::handle_input};
use crate::agent::{Quota, Stats, Usage};
use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::{Terminal, backend::TestBackend};

pub(super) fn new_app() -> App {
    let stats = Stats {
        usage: Usage::default(),
        cache_hit_rate: None,
        tokens_per_second: None,
        context_tokens: 0,
        context_window: 0,
        quota: Quota::default(),
    };
    App::new("model", "medium", stats)
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    handle_input(Event::Key(KeyEvent::from(code)), app, |_| {});
}

pub(super) fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        press(app, KeyCode::Char(character));
    }
}

pub(super) fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn app_with_reply() -> App {
    let mut app = new_app();
    app.push(Role::Assistant, "Earlier reply");
    app
}
