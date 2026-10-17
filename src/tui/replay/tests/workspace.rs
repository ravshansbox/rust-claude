use crate::agent::AgentEvent;
use crate::tui::{
    UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, screen},
};

#[test]
fn updates_branch_in_status_line_after_commands() {
    let finished_tool = |name: &str| {
        UiEvent::Agent(AgentEvent::ToolDone {
            name: name.into(),
            error: None,
            note: None,
        })
    };
    let finished_shell = || UiEvent::Shell("git switch other".into(), true, Ok(String::new()));
    for event in [finished_tool("bash"), finished_shell()] {
        let mut app = new_app();
        app.workspace = "folder · stale-branch".into();
        handle_agent_event(event, &mut app);
        assert!(app.workspace_stale);
        handle_agent_event(UiEvent::Workspace("folder · other".into()), &mut app);
        let screen = screen(&mut app);
        assert!(!screen.contains("stale-branch"));
        assert!(screen.contains("folder · other"));
    }
}

#[test]
fn keeps_status_line_branch_after_file_tools() {
    for name in ["read", "write", "edit"] {
        let mut app = new_app();
        app.workspace = "folder · branch".into();
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolDone {
                name: name.into(),
                error: None,
                note: None,
            }),
            &mut app,
        );
        assert!(!app.workspace_stale, "{name}");
        assert!(screen(&mut app).contains("folder · branch"), "{name}");
    }
}
