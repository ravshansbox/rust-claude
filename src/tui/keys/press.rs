use super::{
    Action, Activity, App, Role,
    editing::edit_input,
    overlays::{handle_history_search_key, handle_picker_key, handle_suggestion_key},
    slash_command::run_command,
};
use crate::{
    models,
    tui::input::{row_above, row_below},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_key(key: KeyEvent, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    let quits =
        key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('c' | 'd'));
    if app.agent_view.is_some() && !quits {
        handle_agent_view_key(key, app);
        return false;
    }
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
        (KeyCode::Char('c'), KeyModifiers::CONTROL) if app.loop_running() => {
            app.stop_loop_after_run();
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
        (KeyCode::Left, KeyModifiers::NONE) if app.input.is_empty() => app.open_agent_view(),
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
        (KeyCode::Enter, _) if app.input.trim() == "/loop" => {
            app.take_input();
            app.toggle_loop();
        }
        (KeyCode::Enter, _)
            if app.can_queue()
                && !app.input.trim().is_empty()
                && !app.input.starts_with('/')
                && !app.input.trim_start().starts_with('!') =>
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

fn handle_agent_view_key(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Esc => app.agent_view = None,
        KeyCode::Left => app.switch_agent(false),
        KeyCode::Right => app.switch_agent(true),
        _ => {
            if let Some(view) = app.agent_view.and_then(|task| app.task_view(task)) {
                scroll(key.code, view);
            }
        }
    }
}

fn scroll(code: KeyCode, app: &mut App) {
    match code {
        KeyCode::Up => app.scroll_up(1),
        KeyCode::Down => app.scroll_down(1),
        KeyCode::PageUp => app.scroll_up(app.page_size),
        KeyCode::PageDown => app.scroll_down(app.page_size),
        KeyCode::Home => app.scroll_to_top(),
        KeyCode::End => app.scroll_to_bottom(),
        _ => {}
    }
}

fn submit_input(app: &mut App, mut act: impl FnMut(Action)) -> bool {
    app.scroll_to_bottom();
    let prompt = app.take_input();
    if let Some(command) = prompt.trim_start().strip_prefix('!') {
        let (command, share) = match command.strip_prefix('!') {
            Some(command) => (command, false),
            None => (command, true),
        };
        let command = command.trim().to_string();
        if share {
            app.remember(&prompt);
        } else {
            app.add_prompt(prompt);
        }
        if !command.is_empty() {
            app.start(Activity::Running);
            act(Action::Shell(command, share));
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
