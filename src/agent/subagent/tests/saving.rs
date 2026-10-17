use super::super::super::{
    Agent,
    test_support::{self, MockApi, Reply, text_reply, tool_reply, tool_result},
};
use super::task;
use serde_json::{Value, json};

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn task_results(agent: &Agent) -> Vec<Value> {
    lines(&test_support::session_file(agent))
        .iter()
        .filter_map(|line| line["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| block["content"].as_array())
        .flatten()
        .cloned()
        .collect()
}

fn subagent_lines(agent: &Agent, result: &Value) -> Vec<Value> {
    let id = result["session"].as_str().unwrap();
    let path = test_support::session_file(agent)
        .with_extension("")
        .join(format!("{id}.jsonl"));
    lines(&path)
}

#[tokio::test]
async fn saves_each_subagent_conversation_in_a_folder_named_after_the_session() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({
                "mode": "sequential",
                "tasks": [task("first", "one"), task("second", "two")],
            }),
        ),
        text_reply("1"),
        text_reply("2"),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let result = agent.prompt("ask", &[], |_| {}).await;
    let results = task_results(&agent);
    let saved: Vec<Vec<Value>> = results
        .iter()
        .map(|result| subagent_lines(&agent, result))
        .collect();
    test_support::remove_session(&agent);
    result.unwrap();
    assert_ne!(results[0]["session"], results[1]["session"]);
    assert_eq!(saved[0][0]["type"], "session");
    assert_eq!(saved[0][1], json!({ "role": "user", "content": "one" }));
    assert_eq!(saved[0][2]["content"][0]["text"], "1");
    assert_eq!(saved[1][2]["content"][0]["text"], "2");
    let requests = api.requests().await;
    assert_eq!(
        tool_result(&requests[3])["content"],
        json!([{ "type": "text", "text": "1" }, { "type": "text", "text": "2" }])
    );
}

#[tokio::test]
async fn saves_the_conversation_of_a_subagent_that_failed() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "agent",
            &json!({ "mode": "sequential", "tasks": [task("first", "one")] }),
        ),
        Reply::BadRequest("bad request".into()),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let result = agent.prompt("ask", &[], |_| {}).await;
    let results = task_results(&agent);
    let saved = subagent_lines(&agent, &results[0]);
    test_support::remove_session(&agent);
    result.unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[1], json!({ "role": "user", "content": "one" }));
}
