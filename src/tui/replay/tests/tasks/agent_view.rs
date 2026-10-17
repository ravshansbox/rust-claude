use super::start_tasks;
use crate::agent::AgentEvent;
use crate::tui::{
    Activity, App, Role, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, press, press_with, screen},
};
use crossterm::event::{KeyCode, KeyModifiers};

fn task_event(app: &mut App, task: usize, event: AgentEvent) {
    handle_agent_event(
        UiEvent::Agent(AgentEvent::TaskEvent {
            task,
            event: Box::new(event),
        }),
        app,
    );
}

fn finish_task(app: &mut App, task: usize) {
    handle_agent_event(
        UiEvent::Agent(AgentEvent::TaskDone {
            task,
            reply: Ok("done".into()),
            tool_calls: 0,
        }),
        app,
    );
}

fn app_with_agents() -> App {
    let mut app = new_app();
    app.push(Role::User, "main prompt");
    app.start(Activity::Working);
    start_tasks(&mut app);
    task_event(&mut app, 0, AgentEvent::Text("hello from one".into()));
    task_event(
        &mut app,
        1,
        AgentEvent::ToolStart {
            name: "bash".into(),
            summary: "ls -la".into(),
            diff: None,
        },
    );
    task_event(&mut app, 1, AgentEvent::Text("hello from two".into()));
    app
}

fn showing(app: &mut App) -> &'static str {
    let screen = screen(app);
    match (
        screen.contains("main prompt"),
        screen.contains("hello from one"),
        screen.contains("hello from two"),
    ) {
        (true, false, false) => "main",
        (false, true, false) => "agent 1",
        (false, false, true) => "agent 2",
        _ => panic!("{screen}"),
    }
}

#[test]
fn shows_a_hint_to_open_the_agents_on_the_status_line() {
    let mut app = app_with_agents();
    let screen = screen(&mut app);
    let status = screen.lines().last().unwrap();
    assert!(status.trim_end().ends_with("· ← agents (2)"), "{screen}");
}

#[test]
fn left_on_an_empty_prompt_shows_the_first_agent() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    let screen = screen(&mut app);
    assert!(screen.contains("hello from one"), "{screen}");
    assert!(!screen.contains("main prompt"), "{screen}");
    assert!(screen.contains("say hi"), "{screen}");
}

#[test]
fn shows_only_the_agent_numbers_as_tabs() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    let screen = screen(&mut app);
    assert_eq!(screen.lines().next().unwrap().trim(), "1   2", "{screen}");
}

#[test]
fn shows_the_description_below_the_tabs() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Right);
    let screen = screen(&mut app);
    assert_eq!(
        screen.lines().nth(1).unwrap().trim(),
        "list files",
        "{screen}"
    );
}

#[test]
fn shows_the_prompt_of_the_agent() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Right);
    let screen = screen(&mut app);
    assert!(screen.contains("list the files"), "{screen}");
}

#[test]
fn shows_the_stats_without_the_description_at_the_bottom() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    let screen = screen(&mut app);
    assert_eq!(
        screen.lines().last().unwrap().trim(),
        "←/→ switch agent · Esc back",
        "{screen}"
    );
}

#[test]
fn shows_the_tool_calls_of_an_agent() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Right);
    let screen = screen(&mut app);
    assert!(screen.contains("ls -la"), "{screen}");
}

#[test]
fn left_moves_the_cursor_when_the_prompt_has_text() {
    let mut app = app_with_agents();
    app.insert("ab");
    press(&mut app, KeyCode::Left);
    assert_eq!(showing(&mut app), "main");
    assert_eq!(app.cursor, 1);
}

#[test]
fn left_does_nothing_without_agents() {
    let mut app = new_app();
    app.push(Role::User, "main prompt");
    press(&mut app, KeyCode::Left);
    assert_eq!(showing(&mut app), "main");
}

#[test]
fn left_and_right_switch_between_agents_and_wrap_around() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    let mut shown = vec![showing(&mut app)];
    for code in [KeyCode::Right, KeyCode::Right, KeyCode::Left, KeyCode::Left] {
        press(&mut app, code);
        shown.push(showing(&mut app));
    }
    assert_eq!(
        shown,
        ["agent 1", "agent 2", "agent 1", "agent 2", "agent 1"]
    );
}

#[test]
fn esc_goes_back_to_the_main_session_without_cancelling() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    let mut actions = 0;
    let quit = press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |_| actions += 1);
    assert!(!quit);
    assert_eq!(actions, 0);
    assert_eq!(app.activity, Some(Activity::Working));
    assert_eq!(showing(&mut app), "main");
}

#[test]
fn hides_the_prompt_in_the_agent_view() {
    let mut app = app_with_agents();
    app.insert("draft");
    app.restore_input(String::new());
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(app.input, "");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.input, "");
}

#[test]
fn moves_to_the_next_agent_when_the_shown_one_finishes() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    finish_task(&mut app, 0);
    assert_eq!(showing(&mut app), "agent 2");
}

#[test]
fn goes_back_to_the_main_session_when_the_last_agent_finishes() {
    let mut app = app_with_agents();
    press(&mut app, KeyCode::Left);
    finish_task(&mut app, 0);
    finish_task(&mut app, 1);
    assert_eq!(showing(&mut app), "main");
    press(&mut app, KeyCode::Left);
    assert_eq!(showing(&mut app), "main");
}

#[test]
fn goes_back_to_the_main_session_when_the_prompt_ends() {
    for end in [UiEvent::Cancelled(Ok(())), UiEvent::Done(Ok(()))] {
        let mut app = app_with_agents();
        press(&mut app, KeyCode::Left);
        handle_agent_event(end, &mut app);
        assert_eq!(showing(&mut app), "main");
    }
}
