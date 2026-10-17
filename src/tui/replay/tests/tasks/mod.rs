use super::texts;
use crate::agent::{AgentEvent, Stats, Usage};
use crate::tools;
use crate::tui::{
    App, UiEvent,
    replay::{handle_agent_event, replay_messages},
    test_support::{new_app, screen},
};
use serde_json::json;

mod agent_view;
mod status_line;

fn agent_input() -> serde_json::Value {
    json!({
        "mode": "parallel",
        "tasks": [
            { "description": "say hi", "prompt": "say hi" },
            { "description": "list files", "prompt": "list the files" },
        ],
    })
}

fn start_tasks(app: &mut App) {
    let input = agent_input();
    handle_agent_event(
        UiEvent::Agent(AgentEvent::ToolStart {
            name: "agent".into(),
            summary: tools::summary("agent", &input),
            diff: tools::diff("agent", &input),
        }),
        app,
    );
    for (task, (description, prompt)) in [("say hi", "say hi"), ("list files", "list the files")]
        .into_iter()
        .enumerate()
    {
        handle_agent_event(
            UiEvent::Agent(AgentEvent::TaskStart {
                task,
                description: description.into(),
                prompt: Some(prompt.into()),
            }),
            app,
        );
    }
}

#[test]
fn shows_each_task_while_it_works() {
    let mut app = new_app();
    start_tasks(&mut app);
    assert_eq!(
        texts(&app),
        [
            "agent parallel, 2 tasks",
            "[agent 1] say hi",
            "[agent 2] list files",
        ]
    );
    let screen = screen(&mut app);
    assert!(screen.contains("[agent 1] say hi"), "{screen}");
}

#[test]
fn shows_how_many_tool_calls_each_task_made() {
    let mut app = new_app();
    start_tasks(&mut app);
    for (task, count) in [(0, 1), (0, 2), (1, 1)] {
        handle_agent_event(
            UiEvent::Agent(AgentEvent::TaskToolCalls { task, count }),
            &mut app,
        );
    }
    handle_agent_event(
        UiEvent::Agent(AgentEvent::TaskDone {
            task: 1,
            reply: Err("This task failed: 400 Bad Request".into()),
            tool_calls: 1,
        }),
        &mut app,
    );
    assert_eq!(
        texts(&app),
        [
            "agent parallel, 2 tasks",
            "[agent 1] say hi · 2 tool calls",
            "[agent 2] list files · 1 tool call\n  This task failed: 400 Bad Request",
        ]
    );
}

#[test]
fn shows_the_stats_of_each_task_and_keeps_them_when_it_finishes() {
    let mut app = new_app();
    start_tasks(&mut app);
    let stats = Stats {
        usage: Usage {
            output: 71_000,
            ..Default::default()
        },
        tokens_per_second: Some(107.0),
        context_tokens: Some(40_000),
        context_window: 1_000_000,
        ..Default::default()
    };
    for event in [
        AgentEvent::TaskToolCalls { task: 0, count: 2 },
        AgentEvent::TaskStats { task: 0, stats },
    ] {
        handle_agent_event(UiEvent::Agent(event), &mut app);
    }
    let shown = "[agent 1] say hi · 2 tool calls · 4%/1m · ↓71k 107t/s";
    assert_eq!(texts(&app)[1], shown);
    handle_agent_event(
        UiEvent::Agent(AgentEvent::TaskDone {
            task: 0,
            reply: Ok("hi".into()),
            tool_calls: 2,
        }),
        &mut app,
    );
    assert_eq!(texts(&app)[1], shown);
}

#[test]
fn hides_final_replies_but_shows_why_a_task_failed_live_and_replayed() {
    let reply = "reply 1\nreply 2";
    let failure = "This task failed: 400 Bad Request\nmore detail";
    let mut live = new_app();
    start_tasks(&mut live);
    handle_agent_event(
        UiEvent::Agent(AgentEvent::TaskDone {
            task: 0,
            reply: Ok(reply.into()),
            tool_calls: 0,
        }),
        &mut live,
    );
    handle_agent_event(
        UiEvent::Agent(AgentEvent::TaskDone {
            task: 1,
            reply: Err(failure.into()),
            tool_calls: 0,
        }),
        &mut live,
    );
    handle_agent_event(
        UiEvent::Agent(AgentEvent::ToolDone {
            name: "agent".into(),
            error: None,
            note: None,
        }),
        &mut live,
    );
    let mut replayed = new_app();
    replay_messages(
        &mut replayed,
        &[
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "1", "name": "agent", "input": agent_input() }] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "1", "content": [
                { "type": "text", "text": reply },
                { "type": "text", "text": failure },
            ], "is_error": false }] }),
        ],
    );
    assert_eq!(
        texts(&live),
        [
            "agent parallel, 2 tasks",
            "[agent 1] say hi",
            "[agent 2] list files\n  This task failed: 400 Bad Request",
        ]
    );
    assert_eq!(texts(&replayed), texts(&live));
}
