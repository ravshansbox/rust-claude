use super::*;

fn describe(event: AgentEvent) -> String {
    match event {
        AgentEvent::TaskStart {
            task, description, ..
        } => format!("{task} start {description}"),
        AgentEvent::TaskToolCalls { task, count } => format!("{task} tool calls {count}"),
        AgentEvent::TaskDone {
            task,
            reply,
            tool_calls,
        } => format!(
            "{task} done {} after {tool_calls}",
            reply.unwrap_or_else(|error| error)
        ),
        AgentEvent::Text(text) => format!("text {text}"),
        AgentEvent::Thinking(text) => format!("thinking {text}"),
        AgentEvent::ToolStart { name, .. } => format!("tool {name}"),
        AgentEvent::ToolDone { name, error, .. } => format!("tool done {name} {error:?}"),
        AgentEvent::Notice(text) => format!("notice {text}"),
        AgentEvent::Stats(_) | AgentEvent::TaskStats { .. } => "stats".into(),
        AgentEvent::Queued(prompt) => format!("queued {prompt}"),
        AgentEvent::TaskEvent { task, event } => format!("{task} {}", describe(*event)),
    }
}

#[tokio::test]
async fn reports_what_each_subagent_does_from_start_to_finish() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("say hi", "say hi"), task("say bye", "say bye")],
            }),
        ),
        tool_reply("call-2", "bash", &json!({ "command": "true" })),
        text_reply("hi"),
        text_reply("bye"),
        text_reply("done"),
    ])
    .await;
    let mut events = Vec::new();
    let (_, result) =
        test_support::run_prompt(&api, "go", |event| events.push(describe(event))).await;
    result.unwrap();
    let tasks: Vec<_> = events
        .iter()
        .skip_while(|event| *event != "tool agent")
        .take_while(|event| !event.starts_with("tool done agent"))
        .filter(|event| *event != "stats")
        .cloned()
        .collect();
    assert_eq!(
        tasks,
        [
            "tool agent",
            "0 start say hi",
            "0 tool bash",
            "0 tool calls 1",
            "0 tool done bash None",
            "0 text hi",
            "0 done hi after 1",
            "1 start say bye",
            "1 text bye",
            "1 done bye after 0",
        ]
    );
}

#[tokio::test]
async fn reports_the_stats_of_each_subagent() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        tool_reply("call-2", "bash", &json!({ "command": "true" })),
        text_reply("one done"),
        text_reply("two done"),
        text_reply("done"),
    ])
    .await;
    let mut last = std::collections::BTreeMap::new();
    let (_, result) = test_support::run_prompt(&api, "go", |event| {
        if let AgentEvent::TaskStats { task, stats } = event {
            last.insert(task, (stats.usage.output, stats.context_tokens.is_some()));
        }
    })
    .await;
    result.unwrap();
    assert_eq!(
        last.into_iter().collect::<Vec<_>>(),
        [(0, (2, true)), (1, (1, true))]
    );
}
