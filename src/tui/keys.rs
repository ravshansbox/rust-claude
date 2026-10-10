use super::{
    Activity, App, Picker, PickerKind, Role,
    input::{
        next_grapheme, next_word_end, previous_grapheme, previous_word_start, row_above, row_below,
    },
};
use crate::{agent::THINKING_LEVELS, images::Image, skills};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

pub(super) enum Action {
    Submit(String, Vec<Image>, &'static str),
    Shell(String),
    PasteImage,
    Compact(&'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    CheckModel(String),
    NewSession,
    ListModels,
    Context,
    Cancel,
}

/// Whether `handle_input` may change what the screen shows for the event.
/// Mouse capture reports every mouse movement, and only scrolling counts.
pub(super) fn may_change_screen(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse) => matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ),
        Event::Key(key) => key.kind == KeyEventKind::Press,
        Event::Paste(_) | Event::Resize(..) => true,
        Event::FocusGained | Event::FocusLost => false,
    }
}

pub(super) fn handle_input(event: Event, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    if let Event::Mouse(mouse) = event {
        match mouse.kind {
            MouseEventKind::ScrollUp => app.scroll_up(3),
            MouseEventKind::ScrollDown => app.scroll_down(3),
            _ => {}
        }
        return false;
    }
    if let Event::Paste(text) = event {
        if let Some(search) = &mut app.history_search {
            search.query.push_str(&text.replace(['\r', '\n'], " "));
            search.selected = 0;
        } else if app.picker.is_none() {
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            app.input.insert_str(app.cursor, &text);
            app.cursor += text.len();
            app.input_changed();
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
                    PickerKind::Thinking => {
                        app.set_thinking_level(&value);
                    }
                    PickerKind::Model => {
                        app.set_model(&value);
                        act(Action::SetModel(value));
                    }
                }
            }
            KeyCode::Esc => app.picker = None,
            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => return true,
            _ => {}
        }
        return false;
    }

    if let Some(search) = &mut app.history_search {
        let count = search.matches().len();
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
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
                    app.input = prompt;
                    app.cursor = app.input.len();
                    app.input_changed();
                }
            }
            (KeyCode::Esc, _) | (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
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
        app.input.insert(app.cursor, '\n');
        app.cursor += 1;
        app.input_changed();
        return false;
    }

    if let Some(suggestions) = app.visible_suggestions() {
        let count = suggestions.items.len();
        app.command_selected = app.command_selected.min(count - 1);
        match key.code {
            KeyCode::Up => {
                app.command_selected = app.command_selected.saturating_sub(1);
                return false;
            }
            KeyCode::Down => {
                app.command_selected = (app.command_selected + 1).min(count - 1);
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
            app.input.clear();
            app.cursor = 0;
            app.input_changed();
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
        (KeyCode::Esc, _) if app.busy() => {
            if app.activity.is_some_and(Activity::can_cancel) {
                app.start(Activity::Cancelling);
                act(Action::Cancel);
            }
        }
        (KeyCode::Esc, _) => return true,
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => return true,
        (KeyCode::BackTab, _) => app.cycle_thinking_level(),
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
            let prompt = std::mem::take(&mut app.input);
            app.cursor = 0;
            app.forget_files();
            app.history_index = None;
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
                        act(Action::Compact(app.thinking_level));
                    }
                    "/thinking" if argument.is_empty() => {
                        app.open_picker(Picker {
                            kind: PickerKind::Thinking,
                            title: "Select thinking level",
                            items: THINKING_LEVELS
                                .iter()
                                .map(|level| (level.to_string(), level.to_string()))
                                .collect(),
                            selected: THINKING_LEVELS
                                .iter()
                                .position(|level| *level == app.thinking_level)
                                .unwrap_or_default(),
                        });
                    }
                    "/thinking" => {
                        if !app.set_thinking_level(argument) {
                            app.restore_input(prompt);
                        }
                    }
                    "/model" if argument.is_empty() => {
                        app.start(Activity::LoadingModels);
                        act(Action::ListModels);
                    }
                    "/model" => {
                        app.start(Activity::CheckingModel);
                        act(Action::CheckModel(argument.to_string()));
                    }
                    "/context" => {
                        app.start(Activity::MeasuringContext);
                        act(Action::Context);
                    }
                    "/resume" => {
                        app.start(Activity::LoadingSessions);
                        act(Action::ListSessions);
                    }
                    _ if let Some((name, arguments)) = skills::parse_command(&prompt) => {
                        if app.skills.iter().any(|skill| skill.name == name) {
                            app.push(Role::Event, format!("[skill] {name}"));
                            if !arguments.is_empty() {
                                app.push(Role::User, arguments);
                            }
                        } else {
                            app.push(Role::User, prompt.clone());
                        }
                        app.remember(&prompt);
                        app.start(Activity::Working);
                        let images = app.take_images(&prompt);
                        act(Action::Submit(prompt, images, app.thinking_level));
                    }
                    command => {
                        app.push(Role::Event, format!("unknown command: {command}"));
                        app.restore_input(prompt);
                    }
                }
                return false;
            }
            app.push(Role::User, prompt.clone());
            app.remember(&prompt);
            app.start(Activity::Working);
            let images = app.take_images(&prompt);
            act(Action::Submit(prompt, images, app.thinking_level));
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
            let start = previous_word_start(&app.input, app.cursor);
            app.input.replace_range(start..app.cursor, "");
            app.cursor = start;
            app.input_changed();
        }
        (KeyCode::Backspace, _) => {
            let start = previous_grapheme(&app.input, app.cursor);
            app.input.replace_range(start..app.cursor, "");
            app.cursor = start;
            app.input_changed();
        }
        (KeyCode::Char(character), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.insert(app.cursor, character);
            app.cursor += character.len_utf8();
            app.input_changed();
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{Action, handle_input, may_change_screen};
    use crate::images::Image;
    use crate::tui::{
        Activity, App, UiEvent, handle_agent_event,
        test_support::{new_app, screen},
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
            (mouse(MouseEventKind::Drag(MouseButton::Left)), false),
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
        let key = |app: &mut App, code| {
            handle_input(Event::Key(KeyEvent::from(code)), app, |_| {});
        };
        handle_input(Event::Paste("a👍🏽❤️".into()), &mut app, |_| {});
        key(&mut app, KeyCode::Left);
        assert_eq!(app.cursor, "a👍🏽".len());
        key(&mut app, KeyCode::Left);
        assert_eq!(app.cursor, "a".len());
        key(&mut app, KeyCode::Right);
        assert_eq!(app.cursor, "a👍🏽".len());
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Backspace);
        assert_eq!(app.input, "a👍🏽");
        key(&mut app, KeyCode::Backspace);
        assert_eq!(app.input, "a");
    }

    #[test]
    fn checks_model_before_selecting_it() {
        let mut app = new_app();
        handle_input(Event::Paste("/model other".into()), &mut app, |_| {});
        let mut checked = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| {
                if let Action::CheckModel(model) = action {
                    checked = Some(model);
                }
            },
        );
        assert_eq!(checked.as_deref(), Some("other"));
        assert_eq!(app.model, "model");
        assert!(app.busy());
    }

    #[test]
    fn keeps_typed_text_after_an_unknown_command_or_thinking_level() {
        for (typed, error) in [
            (
                "/Users/me/proj/main.rs fails, fix it",
                "unknown command: /Users/me/proj/main.rs",
            ),
            ("/thinking loud", "unknown thinking level: loud"),
        ] {
            let mut app = new_app();
            handle_input(Event::Paste(typed.into()), &mut app, |_| {});
            let mut acted = false;
            handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {
                acted = true
            });
            assert!(!acted && !app.busy(), "{typed}");
            assert!(
                app.messages.last().unwrap().text.starts_with(error),
                "{typed}"
            );
            assert_eq!(app.input, typed);
            assert_eq!(app.cursor, typed.len());
            assert_eq!(app.thinking_level, "medium");
        }
    }

    #[test]
    fn esc_cancels_model_loading() {
        for activity in [Activity::LoadingModels, Activity::CheckingModel] {
            let mut app = new_app();
            app.start(activity);
            let mut cancelled = false;
            let quit = handle_input(
                Event::Key(KeyEvent::from(KeyCode::Esc)),
                &mut app,
                |action| {
                    cancelled |= matches!(action, Action::Cancel);
                },
            );
            assert!(!quit && cancelled, "{activity}");
            handle_agent_event(UiEvent::Cancelled(Ok(())), &mut app);
            assert!(!app.busy());
        }
    }

    fn queue_while_working(app: &mut App, prompt: &str) {
        app.start(Activity::Working);
        handle_input(Event::Paste(prompt.into()), app, |_| {});
        let mut submitted = false;
        handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), app, |action| {
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
        handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {});
        assert_eq!(app.input, "/new");
        assert!(app.queued_prompts().is_empty());
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
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Esc)),
            &mut app,
            |action| cancelled |= matches!(action, Action::Cancel),
        );
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
            "rust-claude-tui-history-{}/history.jsonl",
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
        let control = |character| {
            Event::Key(KeyEvent::new(
                KeyCode::Char(character),
                KeyModifiers::CONTROL,
            ))
        };
        let key = |code| Event::Key(KeyEvent::from(code));
        handle_input(control('r'), &mut app, |_| {});
        let search = app.history_search.as_ref().unwrap();
        assert_eq!(search.matches(), [("fix tests", None), ("add docs", None)]);
        handle_input(Event::Paste("FIX".into()), &mut app, |_| {});
        assert_eq!(app.history_search.as_ref().unwrap().matches().len(), 1);
        handle_input(key(KeyCode::Right), &mut app, |_| {});
        let search = app.history_search.as_ref().unwrap();
        let prompts: Vec<&str> = search.matches().iter().map(|(prompt, _)| *prompt).collect();
        assert_eq!(prompts, ["fix tests", "elsewhere fix"]);
        handle_input(key(KeyCode::Down), &mut app, |_| {});
        let mut submitted = false;
        handle_input(key(KeyCode::Enter), &mut app, |action| {
            submitted |= matches!(action, Action::Submit(..));
        });
        assert!(!submitted);
        assert!(app.history_search.is_none());
        assert_eq!(app.input, "elsewhere fix");
        handle_input(control('r'), &mut app, |_| {});
        handle_input(key(KeyCode::Esc), &mut app, |_| {});
        assert!(app.history_search.is_none());
        handle_input(control('r'), &mut app, |_| {});
        handle_input(control('r'), &mut app, |_| {});
        assert!(app.history_search.is_none());
        assert_eq!(app.input, "elsewhere fix");
    }

    #[test]
    fn closes_history_search_when_models_arrive() {
        let mut app = new_app();
        app.add_prompt("earlier".into());
        handle_input(Event::Paste("/model".into()), &mut app, |_| {});
        handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {});
        assert_eq!(app.activity, Some(Activity::LoadingModels));
        handle_input(Event::Paste("/mo".into()), &mut app, |_| {});
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            &mut app,
            |_| {},
        );
        assert!(screen(&mut app).contains("Prompt history"));
        let models = vec!["model".to_string(), "other".to_string()];
        handle_agent_event(UiEvent::Models(Ok(models)), &mut app);
        let shown = screen(&mut app);
        assert!(shown.contains("Select model"), "{shown}");
        assert!(!shown.contains("Prompt history"), "{shown}");
        assert!(!shown.contains("select model"), "{shown}");
        handle_input(Event::Key(KeyEvent::from(KeyCode::Down)), &mut app, |_| {});
        let mut chosen = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| {
                if let Action::SetModel(model) = action {
                    chosen = Some(model);
                }
            },
        );
        assert_eq!(chosen.as_deref(), Some("other"));
        assert!(app.history_search.is_none());
        assert_eq!(app.input, "/mo");
    }

    #[test]
    fn keeps_folder_prompt_history_across_restarts_and_new_sessions() {
        let path = std::env::temp_dir().join(format!(
            "rust-claude-tui-saved-history-{}/history.jsonl",
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
        let press = |app: &mut App, code| {
            handle_input(Event::Key(KeyEvent::from(code)), app, |_| {});
            app.input.clone()
        };
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Up), "second");
        assert_eq!(press(&mut app, KeyCode::Up), "second");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        handle_input(Event::Paste("third".into()), &mut app, |_| {});
        handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {});
        app.activity = None;
        handle_agent_event(UiEvent::NewSession(Ok(())), &mut app);
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            &mut app,
            |_| {},
        );
        let search = app.history_search.take().unwrap();
        assert_eq!(
            search.matches(),
            [("third", None), ("first", None), ("second", None)]
        );
        let mut restarted = new_app();
        restarted.load_history(Some(path.clone()));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(press(&mut restarted, KeyCode::Up), "third");
        assert_eq!(press(&mut restarted, KeyCode::Up), "first");
    }

    #[test]
    fn shows_context_use() {
        let mut app = new_app();
        handle_input(Event::Paste("/context".into()), &mut app, |_| {});
        let mut requested = false;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| requested |= matches!(action, Action::Context),
        );
        assert!(requested);
        assert!(app.busy());
        let context = crate::agent::ContextUse {
            parts: vec![("messages", 10)],
            total: 10,
            window: 0,
        };
        handle_agent_event(UiEvent::Context(context), &mut app);
        assert!(!app.busy());
        assert!(
            app.messages
                .last()
                .unwrap()
                .text
                .starts_with("context: 10 of unknown\n")
        );
    }

    #[test]
    fn runs_shell_commands() {
        let mut app = new_app();
        handle_input(Event::Paste("! ls -a".into()), &mut app, |_| {});
        let mut command = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| {
                if let Action::Shell(text) = action {
                    command = Some(text);
                }
            },
        );
        assert_eq!(command.as_deref(), Some("ls -a"));
        assert_eq!(app.activity, Some(Activity::Running));
        let mut cancelled = false;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Esc)),
            &mut app,
            |action| cancelled |= matches!(action, Action::Cancel),
        );
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
        let quit = handle_input(
            Event::Key(KeyEvent::from(KeyCode::Esc)),
            &mut app,
            |action| {
                cancelled |= matches!(action, Action::Cancel);
            },
        );
        assert!(!quit);
        assert!(!cancelled);
        assert_eq!(app.activity, Some(Activity::LoadingSessions));
    }

    #[test]
    fn ignores_unbound_control_keys() {
        let mut app = new_app();
        app.input = "hello".into();
        for character in ['g', 'd'] {
            let quit = handle_input(
                Event::Key(KeyEvent::new(
                    KeyCode::Char(character),
                    KeyModifiers::CONTROL,
                )),
                &mut app,
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
        let alt = |code| Event::Key(KeyEvent::new(code, KeyModifiers::ALT));
        handle_input(alt(KeyCode::Left), &mut app, |_| {});
        assert_eq!(app.cursor, "one ".len());
        handle_input(alt(KeyCode::Char('b')), &mut app, |_| {});
        assert_eq!(app.cursor, 0);
        handle_input(alt(KeyCode::Right), &mut app, |_| {});
        assert_eq!(app.cursor, "one".len());
        handle_input(alt(KeyCode::Char('f')), &mut app, |_| {});
        assert_eq!(app.cursor, "one two".len());
        assert_eq!(app.input, "one two");
    }

    #[test]
    fn deletes_previous_word() {
        let mut app = new_app();
        handle_input(Event::Paste("one two three".into()), &mut app, |_| {});
        handle_input(Event::Key(KeyEvent::from(KeyCode::Left)), &mut app, |_| {});
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT)),
            &mut app,
            |_| {},
        );
        assert_eq!(app.input, "one two e");
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            &mut app,
            |_| {},
        );
        assert_eq!(app.input, "one e");
        assert_eq!(app.cursor, "one ".len());
    }

    #[test]
    fn edits_input_at_cursor() {
        let mut app = new_app();
        handle_input(Event::Paste("héllo".into()), &mut app, |_| {});
        for code in [KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Left] {
            handle_input(Event::Key(KeyEvent::from(code)), &mut app, |_| {});
        }
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Backspace)),
            &mut app,
            |_| {},
        );
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Char('j'))),
            &mut app,
            |_| {},
        );
        handle_input(Event::Key(KeyEvent::from(KeyCode::Right)), &mut app, |_| {});
        handle_input(Event::Paste("!".into()), &mut app, |_| {});
        assert_eq!(app.input, "jé!llo");
        assert_eq!(app.cursor, "jé!".len());
        let control = |character| {
            Event::Key(KeyEvent::new(
                KeyCode::Char(character),
                KeyModifiers::CONTROL,
            ))
        };
        handle_input(control('a'), &mut app, |_| {});
        assert_eq!(app.cursor, 0);
        handle_input(control('e'), &mut app, |_| {});
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn adds_newline_with_shift_or_alt_enter() {
        let mut app = new_app();
        let mut submitted = false;
        for modifiers in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            handle_input(Event::Paste("a".into()), &mut app, |_| {});
            handle_input(
                Event::Key(KeyEvent::new(KeyCode::Enter, modifiers)),
                &mut app,
                |_| submitted = true,
            );
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
        let key = |code| Event::Key(KeyEvent::from(code));
        handle_input(key(KeyCode::Up), &mut app, |_| {});
        assert_eq!(app.cursor, "one".len());
        handle_input(key(KeyCode::Down), &mut app, |_| {});
        assert_eq!(app.cursor, "one\ntwo".len());
        assert_eq!(app.input, "one\ntwo");
        app.input = "abcdefg".into();
        app.cursor = 6;
        app.input_width = 4;
        handle_input(key(KeyCode::Up), &mut app, |_| {});
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn picks_file_after_at_sign() {
        let mut app = new_app();
        app.set_files(0, vec!["src/agent.rs".into(), "src/main.rs".into()]);
        for character in "read @src".chars() {
            handle_input(
                Event::Key(KeyEvent::from(KeyCode::Char(character))),
                &mut app,
                |_| {},
            );
        }
        handle_input(Event::Key(KeyEvent::from(KeyCode::Down)), &mut app, |_| {});
        let mut submitted = false;
        handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {
            submitted = true
        });
        assert!(!submitted);
        assert_eq!(app.input, "read @src/main.rs ");
        assert_eq!(app.cursor, app.input.len());
        assert!(app.visible_suggestions().is_none());
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
            handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {});
            app.activity = None;
        }
        let press = |app: &mut App, code| {
            handle_input(Event::Key(KeyEvent::from(code)), app, |_| {});
            app.input.clone()
        };
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
        let press = |app: &mut App, code| {
            handle_input(Event::Key(KeyEvent::from(code)), app, |_| {});
            app.input.clone()
        };
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
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            &mut app,
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
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Backspace)),
            &mut app,
            |_| {},
        );
        handle_input(Event::Paste(" look".into()), &mut app, |_| {});
        let mut sent = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| {
                if let Action::Submit(prompt, images, _) = action {
                    sent = Some((prompt, images));
                }
            },
        );
        assert_eq!(
            sent,
            Some(("[image 1][image 2 look".into(), vec![image(1)]))
        );
        assert!(app.images.is_empty());
    }
}
