use super::super::{
    AgentEvent,
    test_support::{self, MockApi, Reply, text_reply, tool_reply, tool_result},
};
use crate::config;
use serde_json::{Value, json};
use std::time::Duration;

mod events;
mod tokens;

fn tool_names(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn runs_a_task_in_a_subagent_and_returns_its_final_reply() {
    let prompt = format!("what is 6 x 7? {}", std::process::id());
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [{ "description": "work out the answer", "prompt": prompt }],
            }),
        ),
        text_reply("42"),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let result = agent.prompt("ask a subagent", &[], |_| {}).await;
    let sessions = config::dir().unwrap().join("sessions");
    let saved_with_prompt = std::fs::read_dir(&sessions)
        .unwrap()
        .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
        .filter(|text| text.contains(&prompt))
        .count();
    test_support::remove_session(&agent);
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let child = &requests[1];
    assert!(tool_names(&requests[0]).contains(&"agent".to_string()));
    assert!(!tool_names(child).contains(&"agent".to_string()));
    assert!(tool_names(child).contains(&"bash".to_string()));
    assert!(
        child["tools"]
            .as_array()
            .unwrap()
            .contains(&json!({ "type": "web_search_20250305", "name": "web_search" })),
        "{}",
        child["tools"]
    );
    assert!(tool_names(child).contains(&"web_fetch".to_string()));
    assert!(tool_names(&requests[0]).contains(&"bash_stop".to_string()));
    assert!(!tool_names(child).contains(&"bash_stop".to_string()));
    let background = |request: &Value| {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "bash")
            .unwrap()["input_schema"]["properties"]
            .get("run_in_background")
            .is_some()
    };
    assert!(background(&requests[0]));
    assert!(!background(child));
    assert_eq!(child["system"], requests[0]["system"]);
    assert_eq!(child["model"], requests[0]["model"]);
    assert_eq!(child["thinking"], requests[0]["thinking"]);
    assert_eq!(child["messages"].as_array().unwrap().len(), 1);
    assert!(child["messages"].to_string().contains(&prompt));
    assert!(!child["messages"].to_string().contains("ask a subagent"));
    let result = tool_result(&requests[2]);
    assert_eq!(result["is_error"], false);
    assert_eq!(result["content"], json!([{ "type": "text", "text": "42" }]));
    assert_eq!(saved_with_prompt, 1);
}

fn task(description: &str, prompt: &str) -> Value {
    json!({ "description": description, "prompt": prompt })
}

#[tokio::test]
async fn gives_each_sequential_task_the_previous_final_reply() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("list files", "list the files"), task("pick one", "pick a file")],
            }),
        ),
        text_reply("a.rs and b.rs"),
        text_reply("b.rs"),
        text_reply("done"),
    ])
    .await;
    let (_, result) = test_support::run_prompt(&api, "chain", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[2]["messages"][0]["content"],
        json!([{
            "type": "text",
            "text": "pick a file\n\n<previous-task-result>\na.rs and b.rs\n</previous-task-result>",
            "cache_control": { "type": "ephemeral" },
        }])
    );
    assert_eq!(
        tool_result(&requests[3])["content"],
        json!([
            { "type": "text", "text": "a.rs and b.rs" },
            { "type": "text", "text": "b.rs" },
        ])
    );
}

#[tokio::test]
async fn returns_the_whole_reply_of_a_subagent_whose_reply_was_paused() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({ "mode": "sequential", "tasks": [task("search", "find it")] }),
        ),
        test_support::stopped_reply("The answer is ", "pause_turn"),
        text_reply("42."),
        text_reply("done"),
    ])
    .await;
    let (_, result) = test_support::run_prompt(&api, "ask", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(
        tool_result(&requests[3])["content"],
        json!([{ "type": "text", "text": "The answer is 42." }])
    );
}

#[tokio::test]
async fn says_when_a_subagent_finished_without_a_reply() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({ "mode": "sequential", "tasks": [task("search", "find it")] }),
        ),
        text_reply(""),
        text_reply("done"),
    ])
    .await;
    let (_, result) = test_support::run_prompt(&api, "ask", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(
        tool_result(&requests[2])["content"],
        json!([{ "type": "text", "text": "This task finished without a reply." }])
    );
}

#[tokio::test]
async fn stops_a_sequence_at_the_task_that_failed() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        Reply::BadRequest("prompt is too long".into()),
        text_reply("done"),
    ])
    .await;
    let (_, result) = test_support::run_prompt(&api, "chain", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let content = tool_result(&requests[2])["content"].clone();
    let first = content[0]["text"].as_str().unwrap();
    assert!(first.starts_with("This task failed: "), "{first}");
    assert!(first.contains("prompt is too long"), "{first}");
    assert_eq!(
        content[1]["text"],
        "This task did not run, because an earlier task failed."
    );
}

#[tokio::test]
async fn runs_parallel_tasks_at_the_same_time() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "parallel",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        Reply::Stall,
        Reply::Stall,
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut prompt = Box::pin(agent.prompt("both", &[], |_| {}));
    let both_sent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut prompt => return false,
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if api.requests().await.len() == 3 {
                        return true;
                    }
                }
            }
        }
    })
    .await;
    drop(prompt);
    test_support::remove_session(&agent);
    assert_eq!(both_sent, Ok(true));
}

#[tokio::test]
async fn rejects_tasks_it_cannot_run() {
    let cases = [
        (
            json!({ "mode": "parallel", "tasks": [] }),
            "tasks must not be empty",
        ),
        (
            json!({ "mode": "random", "tasks": [task("a", "b")] }),
            "unknown mode: random",
        ),
        (
            json!({ "mode": "parallel", "tasks": [{ "prompt": "b" }] }),
            "missing argument: description",
        ),
        (
            json!({ "tasks": [task("a", "b")] }),
            "missing argument: mode",
        ),
    ];
    for (input, error) in cases {
        let api = MockApi::start(vec![
            tool_reply("call-1", "agent", &input),
            text_reply("done"),
        ])
        .await;
        let (_, result) = test_support::run_prompt(&api, "go", |_| {}).await;
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 2, "{input}");
        let result = tool_result(&requests[1]);
        assert_eq!(result["is_error"], true, "{input}");
        assert_eq!(result["content"], error, "{input}");
    }
}
