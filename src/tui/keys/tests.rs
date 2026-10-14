use super::{Action, handle_input, may_change_screen};
use crate::images::Image;
use crate::session::SessionSummary;
use crate::tui::{
    Activity, App, UiEvent, handle_agent_event,
    test_support::{app_with_reply, new_app, press, press_with, screen},
};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::time::{Duration, SystemTime};

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
    assert!(screen(&mut app).contains("model: haiku"));
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
    assert_eq!(app.messages.last().unwrap().text, "model: fable");
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

fn queued_prompts(app: &App) -> Vec<String> {
    app.queue
        .lock()
        .map(|queue| queue.iter().map(|queued| queued.prompt.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn queues_prompts_while_working_and_sends_them_when_done() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    queue_while_working(&mut app, "second");
    assert_eq!(app.input, "");
    assert_eq!(queued_prompts(&app), ["first", "second"]);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    let (prompt, images) = app.send_queued().unwrap();
    assert_eq!(prompt, "first\n\nsecond");
    assert_eq!(images, Vec::<Image>::new());
    assert!(app.busy());
    assert_eq!(queued_prompts(&app), Vec::<String>::new());
    assert_eq!(app.messages.last().unwrap().text, "first\n\nsecond");
}

#[test]
fn does_not_queue_commands() {
    let mut app = new_app();
    app.start(Activity::Working);
    handle_input(Event::Paste("/new".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, "/new");
    assert_eq!(queued_prompts(&app), Vec::<String>::new());
}

#[test]
fn restores_queued_prompts_when_cancelled() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    handle_agent_event(UiEvent::Cancelled(Ok(())), &mut app);
    assert_eq!(app.input, "first\n\ndraft");
    assert_eq!(queued_prompts(&app), Vec::<String>::new());
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
fn restores_queued_prompts_when_the_prompt_fails() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    handle_agent_event(UiEvent::Done(Err(anyhow::anyhow!("overloaded"))), &mut app);
    assert!(!app.busy());
    assert_eq!(app.input, "first");
    assert!(app.send_queued().is_none());
    assert_eq!(app.messages.last().unwrap().text, "error: overloaded");
}

#[test]
fn quits_with_esc_ctrl_c_ctrl_d_or_the_quit_command() {
    for (code, modifiers) in [
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
        (KeyCode::Char('d'), KeyModifiers::CONTROL),
    ] {
        let mut app = new_app();
        assert!(press_with(&mut app, code, modifiers, |_| {}), "{code:?}");
    }
    let mut app = new_app();
    handle_input(Event::Paste("/quit".into()), &mut app, |_| {});
    assert!(press_with(
        &mut app,
        KeyCode::Enter,
        KeyModifiers::NONE,
        |_| {}
    ));
}

#[test]
fn clears_a_draft_instead_of_quitting_on_ctrl_c_or_ctrl_d() {
    let mut app = new_app();
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    assert!(!press_with(
        &mut app,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
        |_| {}
    ));
    assert_eq!(app.input, "draft");
    assert!(!press_with(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        |_| {}
    ));
    assert_eq!(app.input, "");
}

#[test]
fn resumes_the_session_picked_from_the_list() {
    let mut app = new_app();
    let mut listed = false;
    handle_input(Event::Paste("/resume".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        listed |= matches!(action, Action::ListSessions);
    });
    assert!(listed);
    let sessions = [("0", 125), ("1", 3 * 3_600 + 10), ("2", 2 * 86_400 + 10)]
        .map(|(id, age)| SessionSummary {
            id: id.into(),
            modified: SystemTime::now() - Duration::from_secs(age),
            preview: format!("prompt {id}\nmore"),
        })
        .into();
    handle_agent_event(UiEvent::Sessions(Ok(sessions)), &mut app);
    let labels: Vec<&str> = app
        .picker
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|(_, label)| label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "  2m ago  prompt 0",
            "  3h ago  prompt 1",
            "  2d ago  prompt 2"
        ]
    );
    press(&mut app, KeyCode::Down);
    let mut resumed = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Resume(id) = action {
            resumed = Some(id);
        }
    });
    assert_eq!(resumed.as_deref(), Some("1"));
    assert!(app.picker.is_none());
    assert_eq!(app.activity, Some(Activity::Resuming));
}

#[test]
fn resumes_behind_a_background_turn_started_while_picking() {
    let mut app = new_app();
    app.start(Activity::LoadingSessions);
    let sessions = vec![SessionSummary {
        id: "0".into(),
        modified: SystemTime::now(),
        preview: "prompt".into(),
    }];
    handle_agent_event(UiEvent::Sessions(Ok(sessions)), &mut app);
    let message = crate::tools::background::message(1, "echo hi", "hi\n");
    handle_agent_event(UiEvent::Background(message), &mut app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.activity, Some(Activity::Working));
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.activity, Some(Activity::Resuming));
}

#[test]
fn says_when_there_is_no_session_to_resume() {
    let mut app = new_app();
    app.start(Activity::LoadingSessions);
    handle_agent_event(UiEvent::Sessions(Ok(Vec::new())), &mut app);
    assert!(app.picker.is_none());
    assert!(!app.busy());
    assert_eq!(app.messages.last().unwrap().text, "no session to resume");
}

#[test]
fn sets_the_effort_level_picked_from_the_list() {
    let mut app = new_app();
    handle_input(Event::Paste("/effort".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    let picker = app.picker.as_ref().unwrap();
    assert_eq!(picker.items[picker.selected].0, "medium");
    press(&mut app, KeyCode::Down);
    let mut acted = false;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
        acted = true;
    });
    assert!(!acted);
    assert!(app.picker.is_none());
    assert_eq!(app.effort, "high");
    assert_eq!(app.messages.last().unwrap().text, "effort: high");
}

#[test]
fn stops_the_picker_selection_at_either_end_and_closes_it_with_esc() {
    let mut app = new_app();
    handle_input(Event::Paste("/effort".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    let levels: Vec<String> = app
        .picker
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|(level, _)| level.clone())
        .collect();
    let selected = |app: &App| {
        let picker = app.picker.as_ref().unwrap();
        picker.items[picker.selected].0.clone()
    };
    for _ in 0..=levels.len() {
        press(&mut app, KeyCode::Up);
    }
    assert_eq!(selected(&app), levels[0]);
    for _ in 0..=levels.len() {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(&selected(&app), levels.last().unwrap());
    press(&mut app, KeyCode::Esc);
    assert!(!screen(&mut app).contains("Select effort level"));
    assert_eq!(app.effort, "medium");
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
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
        ]
    );
    assert_eq!(app.model, "claude-sonnet-5-5");
    let shown = screen(&mut app);
    assert!(shown.contains("model: opus"), "{shown}");
    assert!(shown.contains("sonnet:medium"), "{shown}");
    assert_eq!(app.input, "");
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
    assert_eq!(rows, ["fable", "opus", "sonnet", "haiku"], "{shown}");
    press(&mut app, KeyCode::Down);
    let mut chosen = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::SetModel(model) = action {
            chosen = Some(model);
        }
    });
    assert_eq!(chosen.as_deref(), Some("claude-sonnet-5-5"));
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
    assert_eq!(app.model, crate::models::DEFAULT);
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
    assert_eq!(app.images, Vec::new());
}

#[test]
fn starts_a_new_session_without_the_earlier_conversation() {
    let mut app = app_with_reply();
    let mut started = false;
    handle_input(Event::Paste("/new".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        started |= matches!(action, Action::NewSession);
    });
    assert!(started);
    assert!(screen(&mut app).contains("starting new session"));
    handle_agent_event(UiEvent::NewSession(Ok(())), &mut app);
    let shown = screen(&mut app);
    assert!(!shown.contains("Earlier reply"), "{shown}");
    assert!(shown.contains("new session"), "{shown}");
    assert!(!shown.contains("starting new session"), "{shown}");
}

#[test]
fn compacts_at_the_chosen_effort_level() {
    let mut app = app_with_reply();
    let mut effort = None;
    handle_input(Event::Paste("/compact".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Compact(level) = action {
            effort = Some(level);
        }
    });
    assert_eq!(effort, Some("medium"));
    assert!(screen(&mut app).contains("compacting"));
}

#[test]
fn types_each_character_once_when_the_terminal_reports_key_releases() {
    let mut app = new_app();
    let mut release = KeyEvent::from(KeyCode::Char('a'));
    release.kind = KeyEventKind::Release;
    press(&mut app, KeyCode::Char('a'));
    handle_input(Event::Key(release), &mut app, |_| {});
    assert_eq!(app.input, "a");
}
