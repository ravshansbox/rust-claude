use super::start_tasks;
use crate::agent::AgentEvent;
use crate::tui::{
    App, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, screen},
};

fn status_line(app: &mut App) -> String {
    screen(app).lines().last().unwrap().trim_end().to_string()
}

#[test]
fn shows_how_many_agents_run_on_the_status_line() {
    let mut app = new_app();
    let idle = status_line(&mut app);
    start_tasks(&mut app);
    let mut shown = vec![status_line(&mut app)];
    for task in 0..2 {
        handle_agent_event(
            UiEvent::Agent(AgentEvent::TaskDone {
                task,
                reply: Ok("done".into()),
                tool_calls: 0,
            }),
            &mut app,
        );
        shown.push(status_line(&mut app));
    }
    assert!(idle.ends_with("opus:medium"), "{idle}");
    assert_eq!(
        shown,
        [
            format!("{idle} · 2 agents"),
            format!("{idle} · 1 agent"),
            idle,
        ]
    );
}

#[test]
fn stops_showing_agents_when_the_prompt_ends() {
    for end in [UiEvent::Cancelled(Ok(())), UiEvent::Done(Ok(()))] {
        let mut app = new_app();
        start_tasks(&mut app);
        handle_agent_event(end, &mut app);
        let screen = screen(&mut app);
        assert!(!screen.contains("agents"), "{screen}");
    }
}
