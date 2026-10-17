use super::super::{Action, handle_input};
use crate::tui::{
    Activity, App, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, press, press_with, screen},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};

fn enter(app: &mut App, text: &str) -> Vec<String> {
    handle_input(Event::Paste(text.into()), app, |_| {});
    let mut submitted = Vec::new();
    press_with(
        app,
        KeyCode::Enter,
        KeyModifiers::NONE,
        |action| match action {
            Action::Submit(prompt, ..) => submitted.push(prompt),
            Action::SubmitLoop(prompt, ..) => submitted.push(format!("loop: {prompt}")),
            _ => {}
        },
    );
    submitted
}

fn footer(app: &mut App) -> String {
    screen(app).lines().last().unwrap().to_string()
}

#[test]
fn turns_loop_mode_on_and_off() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    assert!(screen(&mut app).contains("loop mode on"));
    assert!(footer(&mut app).contains(" · loop"), "{}", footer(&mut app));
    enter(&mut app, "/loop");
    assert!(screen(&mut app).contains("loop mode off"));
    assert!(!footer(&mut app).contains("loop"), "{}", footer(&mut app));
}

#[test]
fn sends_the_next_prompt_again_in_a_new_session_after_each_run() {
    let mut app = new_app();
    app.push(crate::tui::Role::Assistant, "earlier answer");
    enter(&mut app, "/loop");
    assert!(screen(&mut app).contains("earlier answer"));
    assert_eq!(
        enter(&mut app, "do the next task"),
        ["loop: do the next task"]
    );
    let shown = screen(&mut app);
    assert!(!shown.contains("earlier answer"), "{shown}");
    assert!(shown.contains("do the next task"), "{shown}");
    assert!(footer(&mut app).contains("loop 1"), "{}", footer(&mut app));
    app.push(crate::tui::Role::Assistant, "first answer");
    assert_eq!(app.next_loop_round(), None);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    let (prompt, images) = app.next_loop_round().unwrap();
    assert_eq!(prompt, "do the next task");
    assert!(images.is_empty());
    assert!(app.busy());
    let shown = screen(&mut app);
    assert!(!shown.contains("first answer"), "{shown}");
    assert!(shown.contains("do the next task"), "{shown}");
    assert!(footer(&mut app).contains("loop 2"), "{}", footer(&mut app));
}

#[test]
fn waits_for_queued_prompts_between_runs() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    enter(&mut app, "go");
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    app.queue.lock().unwrap().push(crate::agent::Queued {
        prompt: "next".into(),
        images: Vec::new(),
    });
    assert_eq!(app.next_loop_round(), None);
    app.queue.lock().unwrap().clear();
    assert!(app.next_loop_round().is_some());
}

#[test]
fn stops_the_loop_when_a_run_fails() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    enter(&mut app, "go");
    handle_agent_event(UiEvent::Done(Err(anyhow::anyhow!("boom"))), &mut app);
    assert_eq!(app.next_loop_round(), None);
    assert!(screen(&mut app).contains("loop mode off"));
    assert!(!footer(&mut app).contains("loop"), "{}", footer(&mut app));
}

#[test]
fn stops_the_loop_when_cancelled() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    enter(&mut app, "go");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.activity, Some(Activity::Cancelling));
    handle_agent_event(UiEvent::Cancelled(Ok(())), &mut app);
    assert_eq!(app.next_loop_round(), None);
    assert!(!footer(&mut app).contains("loop"), "{}", footer(&mut app));
}

#[test]
fn turns_loop_mode_off_while_a_run_is_going() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    enter(&mut app, "go");
    enter(&mut app, "/loop");
    assert_eq!(app.input, "");
    assert!(app.busy());
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.next_loop_round(), None);
}

#[test]
fn ctrl_c_stops_the_loop_after_the_current_run() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    enter(&mut app, "go");
    let quit = press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL, |_| {});
    assert!(!quit);
    assert!(app.busy());
    assert!(screen(&mut app).contains("loop stops after this run"));
    assert!(!footer(&mut app).contains("loop"), "{}", footer(&mut app));
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.next_loop_round(), None);
    let quit = press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL, |_| {});
    assert!(quit);
}

#[test]
fn ctrl_c_quits_when_loop_mode_waits_for_a_prompt() {
    let mut app = new_app();
    enter(&mut app, "/loop");
    let quit = press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL, |_| {});
    assert!(quit);
}

#[test]
fn shows_a_skill_command_as_outside_loop_mode_in_each_run() {
    let mut app = new_app();
    app.skills = vec![crate::skills::Skill {
        name: "review".into(),
        description: "review skill".into(),
        path: "/skills/review/SKILL.md".into(),
        base_dir: "/skills/review".into(),
        disable_model_invocation: false,
        scope: crate::skills::Scope::Project,
    }];
    enter(&mut app, "/loop");
    assert_eq!(
        enter(&mut app, "/skill:review the diff"),
        ["loop: /skill:review the diff"]
    );
    let texts = |app: &App| -> Vec<String> {
        app.messages
            .iter()
            .map(|message| message.text.clone())
            .collect()
    };
    assert_eq!(texts(&app), ["[skill] review", "the diff"]);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    app.next_loop_round().unwrap();
    assert_eq!(texts(&app), ["[skill] review", "the diff"]);
}
