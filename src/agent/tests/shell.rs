use serde_json::json;
use std::time::Duration;

use crate::{
    agent::{
        AgentEvent,
        test_support::{self, MockApi, text_reply, tool_reply},
    },
    tools::test_support::{TemporaryFile, is_running},
};

#[tokio::test]
async fn sends_a_shell_command_and_its_output_with_the_next_prompt() {
    let api = MockApi::start(vec![text_reply("it printed hi")]).await;
    let mut agent = test_support::agent(&api);
    let output = agent.shell("echo hi").await;
    let result = agent.prompt("what did it print?", &[], |_| {}).await;
    test_support::remove_session(&agent);
    assert_eq!(output.unwrap(), "hi\n");
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(
        requests[0]["messages"][0],
        json!({
            "role": "user",
            "content": "<bash-input>echo hi</bash-input>\n<bash-output>hi\n</bash-output>",
        })
    );
}

#[tokio::test]
async fn runs_a_command_in_the_background_and_gives_its_output_when_it_finishes() {
    let gate = TemporaryFile::new("background-gate", "");
    std::fs::remove_file(gate.path()).unwrap();
    let command = format!(
        "while [ ! -e {} ]; do sleep 0.05; done; echo finished",
        gate.path().display()
    );
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "bash",
            &json!({ "command": command, "run_in_background": true }),
        ),
        text_reply("waiting"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut counts = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent.prompt("watch it", &[], |event| {
            if let AgentEvent::Stats(stats) = event {
                counts.push(stats.background);
            }
        }),
    )
    .await;
    let running = !agent.background.is_empty();
    std::fs::write(gate.path(), "").unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), agent.background.next()).await;
    test_support::remove_session(&agent);
    result.expect("prompt waited for the command").unwrap();
    assert!(running);
    assert_eq!(counts, [0, 0, 1, 1]);
    let requests = api.requests().await;
    let result = test_support::tool_result(&requests[1])["content"].to_string();
    assert!(result.contains("started background task 1"), "{result}");
    let message = message.unwrap().unwrap();
    assert!(message.contains("finished"), "{message}");
    assert!(message.contains(&command), "{message}");
    assert!(agent.background.is_empty());
}

#[tokio::test]
async fn stops_a_background_command() {
    let pid_file = TemporaryFile::new("background-pid", "");
    let command = format!("echo $$ > {}; exec sleep 30", pid_file.path().display());
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "bash",
            &json!({ "command": command, "run_in_background": true }),
        ),
        text_reply("started"),
        tool_reply("call-2", "bash_stop", &json!({ "id": 1 })),
        text_reply("stopped"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let started = agent.prompt("start it", &[], |_| {}).await;
    let pid = pid_file.wait_for_content().await;
    let stopped = agent.prompt("stop it", &[], |_| {}).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    test_support::remove_session(&agent);
    started.unwrap();
    stopped.unwrap();
    let requests = api.requests().await;
    let result = test_support::tool_result(&requests[3])["content"].to_string();
    assert!(result.contains("stopped background task 1"), "{result}");
    assert!(agent.background.is_empty());
    assert!(!is_running(&pid), "process {pid} still running");
}

#[tokio::test]
async fn stops_background_commands_when_a_new_session_starts() {
    let pid_file = TemporaryFile::new("background-new-session-pid", "");
    let command = format!("echo $$ > {}; exec sleep 30", pid_file.path().display());
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "bash",
            &json!({ "command": command, "run_in_background": true }),
        ),
        text_reply("started"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let started = agent.prompt("start it", &[], |_| {}).await;
    let pid = pid_file.wait_for_content().await;
    test_support::remove_session(&agent);
    let new_session = agent.new_session();
    tokio::time::sleep(Duration::from_millis(200)).await;
    started.unwrap();
    new_session.unwrap();
    assert!(agent.background.is_empty());
    assert!(!is_running(&pid), "process {pid} still running");
}
