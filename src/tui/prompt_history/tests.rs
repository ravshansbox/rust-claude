use crate::history;
use crate::tools::test_support::TemporaryDir;
use crate::tui::{
    App, UiEvent,
    keys::{Action, handle_input},
    prompt_history::history_writer,
    replay::handle_agent_event,
    test_support::{new_app, press, press_with},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use std::io::Write;

#[test]
fn deletes_whole_emoji_from_the_history_search_query() {
    let mut app = new_app();
    press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
    handle_input(Event::Paste("a👍🏽❤️".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.history_search.as_ref().unwrap().query, "a👍🏽");
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.history_search.as_ref().unwrap().query, "a");
}

#[test]
fn searches_prompt_history_in_tabs() {
    let folder = TemporaryDir::new("tui-history");
    let path = folder.path().join("history.jsonl");
    std::fs::write(
        &path,
        "{\"prompt\":\"elsewhere fix\",\"cwd\":\"/somewhere/else\"}\n",
    )
    .unwrap();
    let mut app = new_app();
    app.load_history(Some(path));
    for prompt in ["fix tests", "add docs", "fix tests"] {
        app.remember(prompt);
    }
    // Searching uses the history loaded at start-up, not the file.
    drop(folder);
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
fn filters_prompt_history_as_the_user_types() {
    let mut app = new_app();
    for prompt in ["fix tests", "add docs", "fix docs"] {
        app.remember(prompt);
    }
    press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
    let prompts = |app: &App| -> Vec<String> {
        let search = app.history_search.as_ref().unwrap();
        search
            .matches()
            .iter()
            .map(|(prompt, _)| prompt.to_string())
            .collect()
    };
    for character in "Docs".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    assert_eq!(prompts(&app), ["fix docs", "add docs"]);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(prompts(&app), Vec::<String>::new());
    press(&mut app, KeyCode::Enter);
    assert!(app.history_search.is_none());
    assert_eq!(app.input, "");
    press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
    press(&mut app, KeyCode::Char('f'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, "fix docs");
}

#[test]
fn keeps_folder_prompt_history_across_restarts_and_new_sessions() {
    let folder = TemporaryDir::new("tui-saved-history");
    let path = folder.path().join("history.jsonl");
    for prompt in ["first", "second", "first"] {
        history::append(&path, &history::Entry::here(prompt)).unwrap();
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut file| {
            file.write_all(b"{\"prompt\":\"elsewhere\",\"cwd\":\"/somewhere/else\"}\n")
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
    let (sender, writer) = history_writer(tokio::sync::mpsc::unbounded_channel().0);
    app.unsaved_history
        .drain(..)
        .for_each(|entry| sender.send(entry).unwrap());
    drop(sender);
    writer.join().unwrap();
    let mut restarted = new_app();
    restarted.load_history(Some(path));
    drop(folder);
    assert_eq!(press(&mut restarted, KeyCode::Up), "third");
    assert_eq!(press(&mut restarted, KeyCode::Up), "first");
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
