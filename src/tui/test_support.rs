use super::{App, keys::handle_input};
use crate::agent::{Quota, Stats, Usage};
use crossterm::event::{Event, KeyCode, KeyEvent};

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
