use super::{
    UiEvent, prompt_history::history_writer, settings_sync::save_changed_settings,
    test_support::new_app, workspace::git_branch,
};
use crate::{
    agent::AgentEvent, config, history, settings::Settings, tools::test_support::TemporaryDir,
};

#[test]
fn shows_the_branch_of_a_repository_with_no_commits() {
    let directory = TemporaryDir::new("unborn-branch");
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(directory.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(git_branch(directory.path()).as_deref(), Some("main"));
}

#[test]
fn saves_prompt_history_in_order_and_reports_failures() {
    let directory = TemporaryDir::new("history-writer");
    let folder = directory.path();
    let path = folder.join("history.jsonl");
    std::fs::write(folder.join("missing"), "a file, not a folder").unwrap();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (sender, writer) = history_writer(events);
    for prompt in ["one", "two"] {
        sender
            .send((path.clone(), history::Entry::here(prompt)))
            .unwrap();
    }
    sender
        .send((
            folder.join("missing").join("history.jsonl"),
            history::Entry::here("three"),
        ))
        .unwrap();
    drop(sender);
    writer.join().unwrap();
    let saved: Vec<String> = history::load(&path)
        .into_iter()
        .map(|entry| entry.prompt)
        .collect();
    let notice = match received.try_recv() {
        Ok(UiEvent::Agent(AgentEvent::Notice(notice))) => notice,
        _ => String::new(),
    };
    assert_eq!(saved, ["one", "two"]);
    assert!(
        notice.starts_with("failed to save prompt history: "),
        "{notice}"
    );
}

#[test]
fn saves_only_the_model_and_effort_the_user_changed() {
    let path = config::dir().unwrap().join("settings.json");
    std::fs::write(&path, r#"{ "effort": "low", "notifications": true }"#).unwrap();
    let mut app = new_app();
    let (mut saved_model, mut saved_effort) = (app.model.clone(), app.effort);
    app.set_model("claude-haiku-5-5");
    save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
    let after_model = Settings::load();
    app.set_effort("high");
    save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
    let after_effort = Settings::load();
    std::fs::write(&path, "{ broken").unwrap();
    app.set_effort("max");
    save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
    let broken = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(after_model.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(after_model.effort.as_deref(), Some("low"));
    assert_eq!(after_model.notifications, Some(true));
    assert_eq!(after_effort.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(after_effort.effort.as_deref(), Some("high"));
    assert_eq!(broken, "{ broken");
    let last = &app.messages.last().unwrap().text;
    assert!(last.starts_with("failed to save settings: "), "{last}");
}
