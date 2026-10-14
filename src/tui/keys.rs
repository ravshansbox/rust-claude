use super::{
    Activity, App, Picker, PickerKind, Role, display_model,
    input::{
        next_grapheme, next_word_end, previous_grapheme, previous_word_start, row_above, row_below,
    },
};
use crate::{images::Image, models, skills};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;

pub(super) enum Action {
    Submit(String, Vec<Image>, &'static str),
    Shell(String),
    PasteImage,
    Compact(&'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    NewSession,
    McpSignIn(String),
    McpSignOut(String),
    Cancel,
}

/// Whether `handle_input` may change what the screen shows for the event.
/// Mouse capture reports every mouse movement, and only scrolling and the
/// left button count.
pub(super) fn may_change_screen(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse) => matches!(
            mouse.kind,
            MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::Up(MouseButton::Left)
        ),
        Event::Key(key) => key.kind == KeyEventKind::Press,
        Event::Paste(_) | Event::Resize(..) => true,
        Event::FocusGained | Event::FocusLost => false,
    }
}

pub(super) fn handle_input(event: Event, app: &mut App, act: impl FnMut(Action)) -> bool {
    match event {
        Event::FocusGained => app.focused = true,
        Event::FocusLost => app.focused = false,
        Event::Resize(..) => app.selection = None,
        Event::Mouse(mouse) => handle_mouse(mouse, app),
        Event::Paste(text) => handle_paste(&text, app),
        Event::Key(key) if key.kind == KeyEventKind::Press => return handle_key(key, app, act),
        Event::Key(_) => {}
    }
    false
}

fn handle_mouse(mouse: MouseEvent, app: &mut App) {
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        MouseEventKind::Down(MouseButton::Left)
            if app
                .bottom_button
                .contains(Position::new(mouse.column, mouse.row)) =>
        {
            app.scroll_to_bottom();
        }
        MouseEventKind::Down(MouseButton::Left) => app.press_mouse(mouse.column, mouse.row),
        MouseEventKind::Drag(MouseButton::Left) => app.drag_mouse(mouse.column, mouse.row),
        MouseEventKind::Up(MouseButton::Left) => app.release_mouse(mouse.column, mouse.row),
        _ => {}
    }
}

fn handle_paste(text: &str, app: &mut App) {
    if let Some(search) = &mut app.history_search {
        search.query.push_str(&text.replace(['\r', '\n'], " "));
        search.selected = 0;
    } else if app.picker.is_none() {
        app.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
    }
}

fn handle_key(key: KeyEvent, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    if app.picker.is_some() {
        handle_picker_key(key, app, act);
        return false;
    }
    if app.history_search.is_some() {
        handle_history_search_key(key, app);
        return false;
    }
    if key.code == KeyCode::Enter
        && key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
    {
        app.insert("\n");
        return false;
    }
    if handle_suggestion_key(key, app) {
        return false;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) if !app.input.is_empty() => {
            app.restore_input(String::new());
        }
        (KeyCode::Esc, _) if app.busy() => {
            if app.activity.is_some_and(Activity::can_cancel) {
                app.start(Activity::Cancelling);
                act(Action::Cancel);
            }
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => return true,
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => return true,
        (KeyCode::BackTab, _) => app.cycle_effort(),
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            let model = models::next_known(&app.model);
            app.set_model(model);
            act(Action::SetModel(model.to_string()));
        }
        (KeyCode::Up, _) if app.input.is_empty() || app.history_index.is_some() => {
            app.previous_prompt();
        }
        (KeyCode::Down, _) if app.history_index.is_some() => app.next_prompt(),
        (KeyCode::Up, _) => match row_above(&app.input, app.cursor, app.input_width) {
            Some(cursor) => app.cursor = cursor,
            None => app.scroll_up(1),
        },
        (KeyCode::Down, _) => match row_below(&app.input, app.cursor, app.input_width) {
            Some(cursor) => app.cursor = cursor,
            None => app.scroll_down(1),
        },
        (KeyCode::PageUp, _) => app.scroll_up(app.page_size),
        (KeyCode::PageDown, _) => app.scroll_down(app.page_size),
        (KeyCode::Home, _) => app.scroll_to_top(),
        (KeyCode::End, _) => app.scroll_to_bottom(),
        (KeyCode::Enter, _)
            if app.can_queue()
                && !app.input.trim().is_empty()
                && !app.input.starts_with(['/', '!']) =>
        {
            app.queue_prompt();
        }
        (KeyCode::Enter, _) if !app.busy() && !app.input.trim().is_empty() => {
            return submit_input(app, act);
        }
        (KeyCode::Char('v'), KeyModifiers::CONTROL) => act(Action::PasteImage),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => app.open_history_search(),
        _ => edit_input(key, app),
    }
    false
}

fn edit_input(key: KeyEvent, app: &mut App) {
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

fn submit_input(app: &mut App, mut act: impl FnMut(Action)) -> bool {
    app.scroll_to_bottom();
    let prompt = app.take_input();
    if let Some(command) = prompt.strip_prefix('!') {
        let command = command.trim().to_string();
        app.remember(&prompt);
        if !command.is_empty() {
            app.start(Activity::Running);
            act(Action::Shell(command));
        }
        return false;
    }
    if prompt.starts_with('/') {
        return run_command(prompt, app, act);
    }
    app.push(Role::User, prompt.clone());
    act(app.submit(prompt));
    false
}

fn handle_picker_key(key: KeyEvent, app: &mut App, mut act: impl FnMut(Action)) {
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

fn handle_history_search_key(key: KeyEvent, app: &mut App) {
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
fn handle_suggestion_key(key: KeyEvent, app: &mut App) -> bool {
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

fn effort_picker(app: &App) -> Picker {
    let levels = models::effort_levels(&app.model);
    Picker {
        kind: PickerKind::Effort,
        title: "Select effort level",
        items: levels
            .iter()
            .map(|level| (level.to_string(), level.to_string()))
            .collect(),
        selected: levels
            .iter()
            .position(|level| *level == app.effort)
            .unwrap_or_default(),
    }
}

fn model_picker(current: &str) -> Picker {
    Picker {
        kind: PickerKind::Model,
        title: "Select model",
        items: models::known()
            .map(|model| (model.to_string(), display_model(model).to_string()))
            .collect(),
        selected: models::known()
            .position(|model| model == current)
            .unwrap_or_default(),
    }
}

fn mcp_action(argument: &str) -> Option<Action> {
    match argument
        .split_once(char::is_whitespace)
        .map(|(action, server)| (action, server.trim()))
    {
        Some(("login", server)) if !server.is_empty() => {
            Some(Action::McpSignIn(server.to_string()))
        }
        Some(("logout", server)) if !server.is_empty() => {
            Some(Action::McpSignOut(server.to_string()))
        }
        _ => None,
    }
}

fn run_command(prompt: String, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    let trimmed = prompt.trim();
    let (command, argument) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(command, argument)| {
            (command, argument.trim())
        });
    match command {
        "/quit" => return true,
        "/new" => {
            app.start(Activity::StartingNewSession);
            act(Action::NewSession);
        }
        "/compact" => {
            app.start(Activity::Compacting);
            act(Action::Compact(app.effort));
        }
        "/effort" if argument.is_empty() => app.open_picker(effort_picker(app)),
        "/effort" => {
            if !app.set_effort(argument) {
                app.restore_input(prompt);
            }
        }
        "/model" if argument.is_empty() => app.open_picker(model_picker(&app.model)),
        "/model" if models::is_known(argument) => {
            app.set_model(argument);
            act(Action::SetModel(argument.to_string()));
        }
        "/model" => {
            app.push(Role::Event, models::unknown(argument));
            app.restore_input(prompt);
        }
        "/resume" => {
            app.start(Activity::LoadingSessions);
            act(Action::ListSessions);
        }
        "/mcp" => match mcp_action(argument) {
            Some(action) => act(action),
            None => {
                app.push(
                    Role::Event,
                    "usage: /mcp login <server> or /mcp logout <server>",
                );
                app.restore_input(prompt);
            }
        },
        _ if let Some((name, arguments)) = skills::parse_command(&prompt) => {
            if !app.skills.iter().any(|skill| skill.name == name) {
                app.push(Role::Event, format!("unknown skill: {name}"));
                app.restore_input(prompt);
                return false;
            }
            app.push(Role::Event, format!("[skill] {name}"));
            if !arguments.is_empty() {
                app.push(Role::User, arguments);
            }
            act(app.submit(prompt));
        }
        command => {
            app.push(Role::Event, format!("unknown command: {command}"));
            app.restore_input(prompt);
        }
    }
    false
}

#[cfg(test)]
mod tests;
