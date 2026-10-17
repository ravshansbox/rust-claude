use serde_json::json;

use crate::agent::test_support::{self, MockApi, text_reply};

#[tokio::test]
async fn sends_a_shell_command_and_its_output_with_the_next_prompt() {
    let api = MockApi::start(vec![text_reply("it printed hi")]).await;
    let mut agent = test_support::agent(&api);
    let output = agent.shell("echo hi", true).await;
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
async fn keeps_a_local_shell_command_and_its_output_from_the_model() {
    let api = MockApi::start(vec![text_reply("ok")]).await;
    let mut agent = test_support::agent(&api);
    let output = agent.shell("echo hi", false).await;
    let result = agent.prompt("hello", &[], |_| {}).await;
    test_support::remove_session(&agent);
    assert_eq!(output.unwrap(), "hi\n");
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests[0]["messages"].as_array().unwrap().len(), 1);
    assert!(!requests[0].to_string().contains("echo hi"));
}
