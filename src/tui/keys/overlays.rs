use super::{Action, Activity, App, PickerKind};
use crate::tui::input::previous_grapheme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_picker_key(key: KeyEvent, app: &mut App, mut act: impl FnMut(Action)) {
    let Some(picker) = &mut app.picker else {
        return;
    };
    match key.code {
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down => picker.selected = (picker.selected + 1).min(picker.items.len() - 1),
        KeyCode::Enter => {
            let kind = picker.kind;
            let value = std::mem::take(&mut picker.items[picker.selected].0);
            app.picker = None;
            match kind {
                PickerKind::Session => {
                    app.start(Activity::Resuming);
                    act(Action::Resume(value));
                }
                PickerKind::Effort => {
                    app.set_effort(&value);
                }
                PickerKind::Model => {
                    app.set_model(&value);
                    act(Action::SetModel(value));
                }
            }
        }
        KeyCode::Esc => app.picker = None,
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => app.picker = None,
        _ => {}
    }
}

pub(super) fn handle_history_search_key(key: KeyEvent, app: &mut App) {
    let Some(search) = &mut app.history_search else {
        return;
    };
    let count = search.matches().len();
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => search.selected = search.selected.saturating_sub(1),
        (KeyCode::Down, _) => {
            search.selected = (search.selected + 1).min(count.saturating_sub(1));
        }
        (KeyCode::Left | KeyCode::Right, _) => {
            search.all = !search.all;
            search.selected = 0;
        }
        (KeyCode::Backspace, _) => {
            let end = previous_grapheme(&search.query, search.query.len());
            search.query.truncate(end);
            search.selected = 0;
        }
        (KeyCode::Char(character), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
            search.query.push(character);
            search.selected = 0;
        }
        (KeyCode::Enter, _) => {
            let prompt = search
                .matches()
                .get(search.selected)
                .map(|(prompt, _)| prompt.to_string());
            app.history_search = None;
            if let Some(prompt) = prompt {
                app.restore_input(prompt);
            }
        }
        (KeyCode::Esc, _) | (KeyCode::Char('r' | 'c'), KeyModifiers::CONTROL) => {
            app.history_search = None;
        }
        _ => {}
    }
}

/// Handles keys that move through or accept the visible suggestions, and
/// returns whether the key was used up.
pub(super) fn handle_suggestion_key(key: KeyEvent, app: &mut App) -> bool {
    let Some(suggestions) = app.visible_suggestions() else {
        return false;
    };
    let count = suggestions.items.len();
    app.command_selected = app.command_selected.min(count - 1);
    match key.code {
        KeyCode::Up => app.command_selected = (app.command_selected + count - 1) % count,
        KeyCode::Down => app.command_selected = (app.command_selected + 1) % count,
        KeyCode::Enter if suggestions.files => {
            app.accept_suggestion(&suggestions);
            app.input_changed();
        }
        KeyCode::Enter if !app.busy() => {
            app.accept_suggestion(&suggestions);
            return false;
        }
        KeyCode::Tab => {
            app.accept_suggestion(&suggestions);
            app.command_selected = 0;
            if suggestions.files {
                app.input_changed();
            }
        }
        KeyCode::Esc => app.commands_dismissed = true,
        _ => return false,
    }
    true
}
