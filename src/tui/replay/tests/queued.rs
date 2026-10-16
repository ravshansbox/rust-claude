use super::*;

#[test]
fn shows_queued_prompt_when_the_agent_adds_it() {
    let mut app = new_app();
    handle_agent_event(UiEvent::Agent(AgentEvent::Queued("next".into())), &mut app);
    let last = app.messages.last().unwrap();
    assert!(matches!(last.role, Role::User));
    assert_eq!(last.text, "next");
}
