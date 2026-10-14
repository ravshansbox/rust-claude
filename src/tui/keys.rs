use super::{
    Activity, App, PickerKind, Role,
    input::{
        next_grapheme, next_word_end, previous_grapheme, previous_word_start, row_above, row_below,
    },
};
use crate::{images::Image, models};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use overlays::{handle_history_search_key, handle_picker_key, handle_suggestion_key};
use ratatui::layout::Position;
use slash_command::run_command;

mod overlays;
mod slash_command;

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

#[cfg(test)]
mod tests;
