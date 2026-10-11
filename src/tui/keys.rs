use super::{
    Activity, App, Picker, PickerKind, Role, display_model,
    input::{
        next_grapheme, next_word_end, previous_grapheme, previous_word_start, row_above, row_below,
    },
};
use crate::{images::Image, models, skills};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
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

pub(super) fn handle_input(event: Event, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    match event {
        Event::FocusGained => app.focused = true,
        Event::FocusLost => app.focused = false,
        _ => {}
    }
    if let Event::Mouse(mouse) = event {
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
        return false;
    }
    if let Event::Resize(..) = event {
        app.selection = None;
        return false;
    }
    if let Event::Paste(text) = event {
        if let Some(search) = &mut app.history_search {
            search.query.push_str(&text.replace(['\r', '\n'], " "));
            search.selected = 0;
        } else if app.picker.is_none() {
            app.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
        }
        return false;
    }
    let Event::Key(key) = event else { return false };
    if key.kind != KeyEventKind::Press {
        return false;
    }

    if let Some(picker) = &mut app.picker {
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(picker.items.len() - 1),
            KeyCode::Enter => {
                let kind = picker.kind;
                let value = picker.items[picker.selected].0.clone();
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
        return false;
    }

    if let Some(search) = &mut app.history_search {
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
                search.query.pop();
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

    if let Some(suggestions) = app.visible_suggestions() {
        let count = suggestions.items.len();
        app.command_selected = app.command_selected.min(count - 1);
        match key.code {
            KeyCode::Up => {
                app.command_selected = (app.command_selected + count - 1) % count;
                return false;
            }
            KeyCode::Down => {
                app.command_selected = (app.command_selected + 1) % count;
                return false;
            }
            KeyCode::Enter if suggestions.files => {
                app.accept_suggestion(&suggestions);
                app.input_changed();
                return false;
            }
            KeyCode::Enter if !app.busy() => app.accept_suggestion(&suggestions),
            KeyCode::Tab => {
                app.accept_suggestion(&suggestions);
                app.command_selected = 0;
                if suggestions.files {
                    app.input_changed();
                }
                return false;
            }
            KeyCode::Esc => {
                app.commands_dismissed = true;
                return false;
            }
            _ => {}
        }
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
                let (command, argument) = prompt
                    .trim()
                    .split_once(char::is_whitespace)
                    .map_or((prompt.trim(), ""), |(command, argument)| {
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
                    "/effort" if argument.is_empty() => {
                        app.open_picker(Picker {
                            kind: PickerKind::Effort,
                            title: "Select effort level",
                            items: models::effort_levels(&app.model)
                                .iter()
                                .map(|level| (level.to_string(), level.to_string()))
                                .collect(),
                            selected: models::effort_levels(&app.model)
                                .iter()
                                .position(|level| *level == app.effort)
                                .unwrap_or_default(),
                        });
                    }
                    "/effort" => {
                        if !app.set_effort(argument) {
                            app.restore_input(prompt);
                        }
                    }
                    "/model" if argument.is_empty() => {
                        app.open_picker(Picker {
                            kind: PickerKind::Model,
                            title: "Select model",
                            items: models::known()
                                .map(|model| (model.to_string(), display_model(model).to_string()))
                                .collect(),
                            selected: models::known()
                                .position(|model| model == app.model)
                                .unwrap_or_default(),
                        });
                    }
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
                    "/mcp" => match argument.split_once(char::is_whitespace) {
                        Some(("login", server)) if !server.trim().is_empty() => {
                            act(Action::McpSignIn(server.trim().to_string()));
                        }
                        Some(("logout", server)) if !server.trim().is_empty() => {
                            act(Action::McpSignOut(server.trim().to_string()));
                        }
                        _ => {
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
                return false;
            }
            app.push(Role::User, prompt.clone());
            act(app.submit(prompt));
        }
        (KeyCode::Left | KeyCode::Char('b'), KeyModifiers::ALT) => {
            app.cursor = previous_word_start(&app.input, app.cursor);
        }
        (KeyCode::Right | KeyCode::Char('f'), KeyModifiers::ALT) => {
            app.cursor = next_word_end(&app.input, app.cursor);
        }
        (KeyCode::Char('v'), KeyModifiers::CONTROL) => act(Action::PasteImage),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => app.open_history_search(),
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
    false
}

#[cfg(test)]
mod tests {
    use super::{Action, handle_input, may_change_screen};
    use crate::images::Image;
    use crate::skills::{Scope, Skill};
    use crate::tui::{
        Activity, App, UiEvent, handle_agent_event,
        test_support::{new_app, press, press_with, screen},
    };
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    };

    #[test]
    fn redraws_only_for_events_that_may_change_the_screen() {
        let mouse = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 3,
                row: 4,
                modifiers: KeyModifiers::NONE,
            })
        };
        let mut release = KeyEvent::from(KeyCode::Char('a'));
        release.kind = KeyEventKind::Release;
        for (event, redraw) in [
            (mouse(MouseEventKind::Moved), false),
            (mouse(MouseEventKind::Drag(MouseButton::Right)), false),
            (mouse(MouseEventKind::Down(MouseButton::Left)), true),
            (mouse(MouseEventKind::Drag(MouseButton::Left)), true),
            (mouse(MouseEventKind::Up(MouseButton::Left)), true),
            (mouse(MouseEventKind::ScrollUp), true),
            (mouse(MouseEventKind::ScrollDown), true),
            (Event::Key(release), false),
            (Event::Key(KeyEvent::from(KeyCode::Char('a'))), true),
            (Event::FocusLost, false),
            (Event::Paste("a".into()), true),
            (Event::Resize(80, 24), true),
        ] {
            assert_eq!(may_change_screen(&event), redraw, "{event:?}");
        }
    }

    #[test]
    fn edits_emoji_as_whole_graphemes() {
        let mut app = new_app();
        handle_input(Event::Paste("a👍🏽❤️".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Left);
        assert_eq!(app.cursor, "a👍🏽".len());
        press(&mut app, KeyCode::Left);
        assert_eq!(app.cursor, "a".len());
        press(&mut app, KeyCode::Right);
        assert_eq!(app.cursor, "a👍🏽".len());
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.input, "a👍🏽");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.input, "a");
    }

    #[test]
    fn switches_to_a_listed_model_by_id() {
        let mut app = new_app();
        handle_input(
            Event::Paste("/model claude-haiku-5-5".into()),
            &mut app,
            |_| {},
        );
        let mut chosen = None;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::SetModel(model) = action {
                chosen = Some(model);
            }
        });
        assert_eq!(chosen.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(app.model, "claude-haiku-5-5");
        assert!(!app.busy());
        assert!(screen(&mut app).contains("model: haiku-5-5"));
    }

    #[test]
    fn keeps_typed_text_after_an_unknown_command_or_effort_level() {
        for (typed, error) in [
            (
                "/Users/me/proj/main.rs fails, fix it",
                "unknown command: /Users/me/proj/main.rs",
            ),
            ("/effort loud", "unknown effort level: loud"),
            (
                "/model claude-opus-4-5",
                "unknown model: claude-opus-4-5 (options: claude-fable-5-1, claude-opus-5-5, claude-sonnet-5-5, claude-haiku-5-5)",
            ),
            ("/skill:missing fix it", "unknown skill: missing"),
        ] {
            let mut app = new_app();
            handle_input(Event::Paste(typed.into()), &mut app, |_| {});
            let mut acted = false;
            press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
                acted = true;
            });
            assert!(!acted && !app.busy(), "{typed}");
            assert!(
                app.messages.last().unwrap().text.starts_with(error),
                "{typed}"
            );
            assert_eq!(app.input, typed);
            assert_eq!(app.cursor, typed.len());
            assert_eq!(app.effort, "medium");
        }
    }

    #[test]
    fn offers_off_only_for_models_that_can_turn_thinking_off() {
        let mut app = new_app();
        app.model = "claude-haiku-5-5".into();
        app.effort = "max";
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.effort, "off");
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.effort, "low");
        handle_input(Event::Paste("/effort".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        let items: Vec<&str> = app
            .picker
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|(value, _)| value.as_str())
            .collect();
        assert_eq!(items, ["off", "low", "medium", "high", "xhigh", "max"]);

        let mut app = new_app();
        app.model = "claude-opus-5-5".into();
        app.effort = "max";
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.effort, "low");
        handle_input(Event::Paste("/effort off".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.effort, "low");
        assert_eq!(
            app.messages.last().unwrap().text,
            "unknown effort level: off (options: low, medium, high, xhigh, max)"
        );
    }

    #[test]
    fn switches_off_to_the_default_on_a_model_that_cannot_turn_thinking_off() {
        let mut app = new_app();
        app.model = "claude-sonnet-5-5".into();
        app.effort = "off";
        press_with(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL, |_| {});
        assert_eq!(app.model, "claude-haiku-5-5");
        assert_eq!(app.effort, "off");
        press_with(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL, |_| {});
        assert_eq!(app.model, "claude-fable-5-1");
        assert_eq!(app.effort, "medium");
        assert_eq!(app.messages.last().unwrap().text, "model: fable-5-1");
    }

    #[test]
    fn reports_the_chosen_effort_level() {
        let mut app = new_app();
        handle_input(Event::Paste("/effort high".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.effort, "high");
        assert_eq!(app.messages.last().unwrap().text, "effort: high");
    }

    fn queue_while_working(app: &mut App, prompt: &str) {
        app.start(Activity::Working);
        handle_input(Event::Paste(prompt.into()), app, |_| {});
        let mut submitted = false;
        press_with(app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            submitted |= matches!(action, Action::Submit(..));
        });
        assert!(!submitted);
    }

    #[test]
    fn queues_prompts_while_working_and_sends_them_when_done() {
        let mut app = new_app();
        queue_while_working(&mut app, "first");
        queue_while_working(&mut app, "second");
        assert!(app.input.is_empty());
        assert_eq!(app.queued_prompts(), ["first", "second"]);
        handle_agent_event(UiEvent::Done(Ok(())), &mut app);
        let (prompt, images) = app.send_queued().unwrap();
        assert_eq!(prompt, "first\n\nsecond");
        assert!(images.is_empty());
        assert!(app.busy());
        assert!(app.queued_prompts().is_empty());
        assert_eq!(app.messages.last().unwrap().text, "first\n\nsecond");
    }

    #[test]
    fn does_not_queue_commands() {
        let mut app = new_app();
        app.start(Activity::Working);
        handle_input(Event::Paste("/new".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.input, "/new");
        assert!(app.queued_prompts().is_empty());
    }

    #[test]
    fn lists_commands_and_skill_commands_in_alphabetical_order() {
        let mut app = new_app();
        app.skills = vec![Skill {
            name: "lint".into(),
            description: "lint skill".into(),
            path: "/skills/lint/SKILL.md".into(),
            base_dir: "/skills/lint".into(),
            disable_model_invocation: false,
            scope: Scope::Global,
        }];
        handle_input(Event::Paste("/".into()), &mut app, |_| {});
        let shown = screen(&mut app);
        let names = [
            "/compact",
            "/effort",
            "/mcp",
            "/model",
            "/new",
            "/quit",
            "/resume",
            "/skill:lint",
        ];
        let positions: Vec<usize> = names
            .iter()
            .map(|name| shown.find(&format!("{name} ")).expect(name))
            .collect();
        assert!(positions.is_sorted(), "{shown}");
    }

    fn suggested(app: &App) -> Vec<String> {
        app.visible_suggestions()
            .map(|suggestions| {
                suggestions
                    .items
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn completes_mcp_subcommands_and_server_names() {
        let mut app = new_app();
        app.mcp_sign_in_servers = vec!["figma".into(), "files".into(), "linear".into()];
        handle_input(Event::Paste("/mc".into()), &mut app, |_| {});
        assert_eq!(suggested(&app), ["/mcp"]);
        handle_input(Event::Paste("p ".into()), &mut app, |_| {});
        assert_eq!(suggested(&app), ["/mcp login", "/mcp logout"]);
        handle_input(Event::Paste("login f".into()), &mut app, |_| {});
        assert_eq!(suggested(&app), ["/mcp login figma", "/mcp login files"]);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input, "/mcp login figma");
        let mut signing_in = Vec::new();
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::McpSignIn(name) = action {
                signing_in.push(name);
            }
        });
        assert_eq!(signing_in, ["figma"]);
        assert!(app.input.is_empty());
        handle_input(Event::Paste("/mcp logout l".into()), &mut app, |_| {});
        assert_eq!(suggested(&app), ["/mcp logout linear"]);
        let mut signing_out = Vec::new();
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::McpSignOut(name) = action {
                signing_out.push(name);
            }
        });
        assert_eq!(signing_out, ["linear"]);
    }

    #[test]
    fn explains_how_to_use_the_mcp_command() {
        let mut app = new_app();
        for input in ["/mcp", "/mcp login", "/mcp logout", "/mcp dance figma"] {
            handle_input(Event::Paste(input.into()), &mut app, |_| {});
            app.commands_dismissed = true;
            press(&mut app, KeyCode::Enter);
            assert_eq!(
                app.messages.last().unwrap().text,
                "usage: /mcp login <server> or /mcp logout <server>",
                "{input}"
            );
            assert_eq!(app.input, input);
            app.input.clear();
            app.cursor = 0;
        }
    }

    #[test]
    fn wraps_the_command_selection_at_either_end() {
        let mut app = new_app();
        handle_input(Event::Paste("/".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input, "/resume");
        app.input.clear();
        app.cursor = 0;
        handle_input(Event::Paste("/".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input, "/compact");
    }

    #[test]
    fn runs_the_skill_named_exactly_when_a_longer_name_comes_first() {
        let skill = |name: &str, scope| Skill {
            name: name.into(),
            description: format!("{name} skill"),
            path: format!("/skills/{name}/SKILL.md").into(),
            base_dir: format!("/skills/{name}").into(),
            disable_model_invocation: false,
            scope,
        };
        let mut app = new_app();
        app.skills = vec![
            skill("pdf-tools", Scope::Project),
            skill("pdf", Scope::Global),
        ];
        handle_input(Event::Paste("/skill:pdf".into()), &mut app, |_| {});
        let mut submitted = None;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::Submit(prompt, _, _) = action {
                submitted = Some(prompt);
            }
        });
        assert_eq!(submitted.as_deref(), Some("/skill:pdf"));
        assert_eq!(app.messages.last().unwrap().text, "[skill] pdf");
    }

    #[test]
    fn restores_queued_prompts_when_cancelled() {
        let mut app = new_app();
        queue_while_working(&mut app, "first");
        handle_input(Event::Paste("draft".into()), &mut app, |_| {});
        handle_agent_event(UiEvent::Cancelled(Ok(())), &mut app);
        assert_eq!(app.input, "first\n\ndraft");
        assert!(app.queued_prompts().is_empty());
        assert!(app.send_queued().is_none());
    }

    #[test]
    fn restores_queued_prompts_when_esc_comes_as_the_reply_finishes() {
        let mut app = new_app();
        queue_while_working(&mut app, "first");
        let mut cancelled = false;
        press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |action| {
            cancelled |= matches!(action, Action::Cancel);
        });
        assert!(cancelled);
        handle_agent_event(UiEvent::Done(Ok(())), &mut app);
        assert!(!app.busy());
        assert_eq!(app.input, "first");
        assert!(app.send_queued().is_none());
        assert_eq!(app.messages.last().unwrap().text, "cancelled");
    }

    #[test]
    fn searches_prompt_history_in_tabs() {
        let path = std::env::temp_dir().join(format!(
            "rust-coding-agent-tui-history-{}/history.jsonl",
            std::process::id()
        ));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "{\"prompt\":\"elsewhere fix\",\"cwd\":\"/somewhere/else\"}\n",
        )
        .unwrap();
        let mut app = new_app();
        app.load_history(Some(path.clone()));
        for prompt in ["fix tests", "add docs", "fix tests"] {
            app.remember(prompt);
        }
        // Searching uses the history loaded at start-up, not the file.
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
        let search = app.history_search.as_ref().unwrap();
        assert_eq!(search.matches(), [("fix tests", None), ("add docs", None)]);
        handle_input(Event::Paste("FIX".into()), &mut app, |_| {});
        assert_eq!(app.history_search.as_ref().unwrap().matches().len(), 1);
        press(&mut app, KeyCode::Right);
        let search = app.history_search.as_ref().unwrap();
        let prompts: Vec<&str> = search.matches().iter().map(|(prompt, _)| *prompt).collect();
        assert_eq!(prompts, ["fix tests", "elsewhere fix"]);
        press(&mut app, KeyCode::Down);
        let mut submitted = false;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            submitted |= matches!(action, Action::Submit(..));
        });
        assert!(!submitted);
        assert!(app.history_search.is_none());
        assert_eq!(app.input, "elsewhere fix");
        press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
        press(&mut app, KeyCode::Esc);
        assert!(app.history_search.is_none());
        press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
        press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
        assert!(app.history_search.is_none());
        assert_eq!(app.input, "elsewhere fix");
    }

    #[test]
    fn ctrl_p_switches_between_the_latest_models() {
        let mut app = new_app();
        let mut switched = Vec::new();
        for _ in 0..5 {
            press_with(
                &mut app,
                KeyCode::Char('p'),
                KeyModifiers::CONTROL,
                |action| {
                    if let Action::SetModel(model) = action {
                        switched.push(model);
                    }
                },
            );
        }
        assert_eq!(
            switched,
            [
                "claude-fable-5-1",
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-haiku-5-5",
                "claude-fable-5-1",
            ]
        );
        assert_eq!(app.model, "claude-fable-5-1");
        let shown = screen(&mut app);
        assert!(shown.contains("model: haiku-5-5"), "{shown}");
        assert!(shown.contains("fable-5-1:medium"), "{shown}");
        assert!(app.input.is_empty());
    }

    #[test]
    fn offers_only_the_listed_models_in_the_model_picker() {
        let mut app = new_app();
        let mut acted = false;
        handle_input(Event::Paste("/model".into()), &mut app, |_| {});
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
            acted = true;
        });
        assert!(!acted && !app.busy());
        let shown = screen(&mut app);
        let rows: Vec<&str> = shown
            .lines()
            .skip_while(|line| !line.starts_with("Select model"))
            .skip(1)
            .take_while(|line| !line.starts_with('─'))
            .map(str::trim)
            .collect();
        assert_eq!(
            rows,
            ["fable-5-1", "opus-5-5", "sonnet-5-5", "haiku-5-5"],
            "{shown}"
        );
        press(&mut app, KeyCode::Down);
        let mut chosen = None;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::SetModel(model) = action {
                chosen = Some(model);
            }
        });
        assert_eq!(chosen.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn keeps_folder_prompt_history_across_restarts_and_new_sessions() {
        let path = std::env::temp_dir().join(format!(
            "rust-coding-agent-tui-saved-history-{}/history.jsonl",
            std::process::id()
        ));
        for prompt in ["first", "second", "first"] {
            crate::history::append(&path, &crate::history::Entry::here(prompt)).unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                std::io::Write::write_all(
                    &mut file,
                    b"{\"prompt\":\"elsewhere\",\"cwd\":\"/somewhere/else\"}\n",
                )
            })
            .unwrap();
        let mut app = new_app();
        app.load_history(Some(path.clone()));
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Up), "second");
        assert_eq!(press(&mut app, KeyCode::Up), "second");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        handle_input(Event::Paste("third".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        app.activity = None;
        handle_agent_event(UiEvent::NewSession(Ok(())), &mut app);
        press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
        let search = app.history_search.take().unwrap();
        assert_eq!(
            search.matches(),
            [("third", None), ("first", None), ("second", None)]
        );
        let (sender, writer) = crate::tui::history_writer(tokio::sync::mpsc::unbounded_channel().0);
        app.unsaved_history
            .drain(..)
            .for_each(|entry| sender.send(entry).unwrap());
        drop(sender);
        writer.join().unwrap();
        let mut restarted = new_app();
        restarted.load_history(Some(path.clone()));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(press(&mut restarted, KeyCode::Up), "third");
        assert_eq!(press(&mut restarted, KeyCode::Up), "first");
    }

    #[test]
    fn runs_shell_commands() {
        let mut app = new_app();
        handle_input(Event::Paste("! ls -a".into()), &mut app, |_| {});
        let mut command = None;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::Shell(text) = action {
                command = Some(text);
            }
        });
        assert_eq!(command.as_deref(), Some("ls -a"));
        assert_eq!(app.activity, Some(Activity::Running));
        let mut cancelled = false;
        press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |action| {
            cancelled |= matches!(action, Action::Cancel);
        });
        assert!(cancelled);
        handle_agent_event(
            UiEvent::Shell("ls -a".into(), Ok("a\nb\n".into())),
            &mut app,
        );
        assert!(!app.busy());
        assert_eq!(app.messages.last().unwrap().text, "! ls -a\na\nb");
    }

    #[test]
    fn ignores_escape_while_loading_sessions() {
        let mut app = new_app();
        app.start(Activity::LoadingSessions);
        let mut cancelled = false;
        let quit = press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |action| {
            cancelled |= matches!(action, Action::Cancel);
        });
        assert!(!quit);
        assert!(!cancelled);
        assert_eq!(app.activity, Some(Activity::LoadingSessions));
    }

    #[test]
    fn ignores_unbound_control_keys() {
        let mut app = new_app();
        app.input = "hello".into();
        for character in ['g', 'd'] {
            let quit = press_with(
                &mut app,
                KeyCode::Char(character),
                KeyModifiers::CONTROL,
                |_| {},
            );
            assert!(!quit);
        }
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn moves_by_word_with_alt() {
        let mut app = new_app();
        handle_input(Event::Paste("one two".into()), &mut app, |_| {});
        press_with(&mut app, KeyCode::Left, KeyModifiers::ALT, |_| {});
        assert_eq!(app.cursor, "one ".len());
        press_with(&mut app, KeyCode::Char('b'), KeyModifiers::ALT, |_| {});
        assert_eq!(app.cursor, 0);
        press_with(&mut app, KeyCode::Right, KeyModifiers::ALT, |_| {});
        assert_eq!(app.cursor, "one".len());
        press_with(&mut app, KeyCode::Char('f'), KeyModifiers::ALT, |_| {});
        assert_eq!(app.cursor, "one two".len());
        assert_eq!(app.input, "one two");
    }

    #[test]
    fn deletes_previous_word() {
        let mut app = new_app();
        handle_input(Event::Paste("one two three".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Left);
        press_with(&mut app, KeyCode::Backspace, KeyModifiers::ALT, |_| {});
        assert_eq!(app.input, "one two e");
        press_with(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL, |_| {});
        assert_eq!(app.input, "one e");
        assert_eq!(app.cursor, "one ".len());
    }

    #[test]
    fn edits_input_at_cursor() {
        let mut app = new_app();
        handle_input(Event::Paste("héllo".into()), &mut app, |_| {});
        for code in [KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Left] {
            press(&mut app, code);
        }
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Right);
        handle_input(Event::Paste("!".into()), &mut app, |_| {});
        assert_eq!(app.input, "jé!llo");
        assert_eq!(app.cursor, "jé!".len());
        press_with(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL, |_| {});
        assert_eq!(app.cursor, 0);
        press_with(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL, |_| {});
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn adds_newline_with_shift_or_alt_enter() {
        let mut app = new_app();
        let mut submitted = false;
        for modifiers in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            handle_input(Event::Paste("a".into()), &mut app, |_| {});
            press_with(&mut app, KeyCode::Enter, modifiers, |_| submitted = true);
        }
        assert!(!submitted);
        assert_eq!(app.input, "a\na\n");
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn moves_between_input_lines_with_up_and_down() {
        let mut app = new_app();
        app.prompt_history.push("earlier".into());
        handle_input(Event::Paste("one\ntwo".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Up);
        assert_eq!(app.cursor, "one".len());
        press(&mut app, KeyCode::Down);
        assert_eq!(app.cursor, "one\ntwo".len());
        assert_eq!(app.input, "one\ntwo");
        app.input = "abcdefg".into();
        app.cursor = 6;
        app.input_width = 4;
        press(&mut app, KeyCode::Up);
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn picks_file_after_at_sign() {
        let mut app = new_app();
        app.set_files(0, vec!["src/agent.rs".into(), "src/main.rs".into()]);
        for character in "read @src".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Down);
        let mut submitted = false;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
            submitted = true;
        });
        assert!(!submitted);
        assert_eq!(app.input, "read @src/main.rs ");
        assert_eq!(app.cursor, app.input.len());
        assert!(app.visible_suggestions().is_none());
    }

    #[test]
    fn ctrl_c_closes_pickers_and_history_search_keeping_the_draft() {
        let mut app = new_app();
        app.add_prompt("fix tests".into());
        handle_input(Event::Paste("draft".into()), &mut app, |_| {});
        press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
        handle_input(Event::Paste("fix".into()), &mut app, |_| {});
        assert!(!press_with(
            &mut app,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            |_| {}
        ));
        assert!(!screen(&mut app).contains("Prompt history"));
        assert_eq!(app.input, "draft");
        press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL, |_| {});
        handle_input(Event::Paste("/model".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        assert!(screen(&mut app).contains("Select model"));
        assert!(!press_with(
            &mut app,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            |_| {}
        ));
        assert!(!screen(&mut app).contains("Select model"));
        assert_eq!(app.model, "model");
    }

    #[test]
    fn replaces_the_whole_file_name_when_the_cursor_is_inside_it() {
        let mut app = new_app();
        app.set_files(0, vec!["src/main.rs".into()]);
        handle_input(Event::Paste("read @src/main.rs".into()), &mut app, |_| {});
        for _ in 0..2 {
            press(&mut app, KeyCode::Left);
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.input, "read @src/main.rs ");
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn picks_the_file_named_exactly_before_longer_paths() {
        let mut app = new_app();
        app.set_files(
            0,
            vec![
                "crates/foo/src/lib.rs".into(),
                "crates/src/lib.rs.bak".into(),
                "src/lib.rs".into(),
                "src/lib.rs.orig".into(),
            ],
        );
        handle_input(Event::Paste("explain @SRC/lib".into()), &mut app, |_| {});
        let items: Vec<String> = app
            .visible_suggestions()
            .unwrap()
            .items
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(
            items,
            [
                "src/lib.rs",
                "src/lib.rs.orig",
                "crates/foo/src/lib.rs",
                "crates/src/lib.rs.bak"
            ]
        );
        handle_input(Event::Paste(".rs".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.input, "explain @src/lib.rs ");
    }

    #[test]
    fn pastes_multiple_lines_without_submitting() {
        let mut app = new_app();
        let mut submitted = false;
        let quit = handle_input(Event::Paste("first\r\nsecond".into()), &mut app, |action| {
            submitted |= matches!(action, Action::Submit(..));
        });
        assert!(!quit);
        assert!(!submitted);
        assert_eq!(app.input, "first\nsecond");
    }

    #[test]
    fn walks_through_prompt_history() {
        let mut app = new_app();
        for prompt in ["first", "second"] {
            handle_input(Event::Paste(prompt.into()), &mut app, |_| {});
            press(&mut app, KeyCode::Enter);
            app.activity = None;
        }
        assert_eq!(press(&mut app, KeyCode::Up), "second");
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Down), "second");
        assert_eq!(press(&mut app, KeyCode::Down), "");
        app.input = "draft".into();
        assert_eq!(press(&mut app, KeyCode::Up), "draft");
    }

    #[test]
    fn browses_prompt_history_past_a_recalled_command() {
        let mut app = new_app();
        app.set_files(0, vec!["src/main.rs".into()]);
        for prompt in ["first", "/compact", "read @src", "hello"] {
            app.add_prompt(prompt.into());
        }
        assert_eq!(press(&mut app, KeyCode::Up), "hello");
        assert_eq!(press(&mut app, KeyCode::Up), "read @src");
        assert_eq!(press(&mut app, KeyCode::Up), "/compact");
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Down), "/compact");
        assert_eq!(press(&mut app, KeyCode::Down), "read @src");
        assert_eq!(press(&mut app, KeyCode::Down), "hello");
        assert_eq!(press(&mut app, KeyCode::Up), "read @src");
        assert_eq!(press(&mut app, KeyCode::Backspace), "read @sr");
        assert!(app.visible_suggestions().is_some());
    }

    #[test]
    fn sends_pasted_images_whose_markers_remain() {
        let mut app = new_app();
        let mut pasting = false;
        press_with(
            &mut app,
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
            |action| pasting = matches!(action, Action::PasteImage),
        );
        assert!(pasting);
        let image = |byte| Image {
            media_type: "image/png",
            data: vec![byte],
        };
        handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(1)))), &mut app);
        handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(2)))), &mut app);
        assert_eq!(app.input, "[image 1][image 2]");
        press(&mut app, KeyCode::Backspace);
        handle_input(Event::Paste(" look".into()), &mut app, |_| {});
        let mut sent = None;
        press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
            if let Action::Submit(prompt, images, _) = action {
                sent = Some((prompt, images));
            }
        });
        assert_eq!(
            sent,
            Some(("[image 1][image 2 look".into(), vec![image(1)]))
        );
        assert!(app.images.is_empty());
    }
}
