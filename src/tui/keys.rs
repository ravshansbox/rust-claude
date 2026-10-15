use super::{Activity, App, PickerKind, Role};
use crate::images::Image;
use crossterm::event::{Event, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use press::handle_key;
use ratatui::layout::Position;

mod editing;
mod overlays;
mod press;
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

#[cfg(test)]
mod tests;
