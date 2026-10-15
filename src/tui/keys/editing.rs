use super::App;
use crate::tui::input::{next_grapheme, next_word_end, previous_grapheme, previous_word_start};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn edit_input(key: KeyEvent, app: &mut App) {
    match (key.code, key.modifiers) {
        (KeyCode::Left | KeyCode::Char('b'), KeyModifiers::ALT) => {
            app.cursor = previous_word_start(&app.input, app.cursor);
        }
        (KeyCode::Right | KeyCode::Char('f'), KeyModifiers::ALT) => {
            app.cursor = next_word_end(&app.input, app.cursor);
        }
        (KeyCode::Char('a'), KeyModifiers::CONTROL) => app.cursor = 0,
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => app.cursor = app.input.len(),
        (KeyCode::Left, _) => app.cursor = previous_grapheme(&app.input, app.cursor),
        (KeyCode::Right, _) => app.cursor = next_grapheme(&app.input, app.cursor),
        (KeyCode::Backspace, KeyModifiers::ALT) | (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
            app.delete_to(previous_word_start(&app.input, app.cursor));
        }
        (KeyCode::Backspace, _) => {
            app.delete_to(previous_grapheme(&app.input, app.cursor));
        }
        (KeyCode::Char(character), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.insert(character.encode_utf8(&mut [0; 4]));
        }
        _ => {}
    }
}
