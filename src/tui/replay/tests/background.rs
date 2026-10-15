use super::*;

#[test]
fn shows_queued_prompt_when_the_agent_adds_it() {
    let mut app = new_app();
    handle_agent_event(UiEvent::Agent(AgentEvent::Queued("next".into())), &mut app);
    let last = app.messages.last().unwrap();
    assert!(matches!(last.role, Role::User));
    assert_eq!(last.text, "next");
}

#[test]
fn shows_a_finished_background_command_while_working_on_it() {
    let message = tools::background::message(1, "echo hi", "hi\n");
    let mut app = new_app();
    handle_agent_event(UiEvent::Background(message.clone()), &mut app);
    assert!(app.busy());
    assert!(matches!(app.messages.last().unwrap().role, Role::Tool));
    assert_eq!(
        app.messages.last().unwrap().text,
        "background task 1 finished\nhi"
    );
    let mut resumed = new_app();
    replay_messages(
        &mut resumed,
        &[json!({ "role": "user", "content": message })],
    );
    assert_eq!(
        resumed.messages.last().unwrap().text,
        "background task 1 finished\nhi"
    );
    assert_eq!(resumed.prompt_history, Vec::<String>::new());
}

#[test]
fn stays_busy_with_a_prompt_sent_as_a_background_turn_starts() {
    let mut app = new_app();
    let mut sent = false;
    handle_input(Event::Paste("hello".into()), &mut app, |_| {});
    handle_input(
        Event::Key(KeyEvent::from(KeyCode::Enter)),
        &mut app,
        |action| sent |= matches!(action, Action::Submit(..)),
    );
    let message = tools::background::message(1, "echo hi", "hi\n");
    handle_agent_event(UiEvent::Background(message), &mut app);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert!(sent);
    assert_eq!(app.activity, Some(Activity::Working));
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert!(!app.busy());
}
