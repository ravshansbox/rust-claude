use serde_json::json;

use crate::agent::{
    AgentEvent,
    test_support::{self, MockApi, text_reply, tool_reply},
};

#[tokio::test]
async fn tells_the_model_a_tool_it_named_does_not_exist() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "grep", &json!({ "pattern": "fn" })),
        text_reply("sorry"),
    ])
    .await;
    let mut errors = Vec::new();
    let (_, result) = test_support::run_prompt(&api, "search", |event| {
        if let AgentEvent::ToolDone { name, error, .. } = event {
            errors.push((name, error));
        }
    })
    .await;
    result.unwrap();
    assert_eq!(
        errors,
        [("grep".to_string(), Some("unknown tool: grep".to_string()))]
    );
    let requests = api.requests().await;
    let result = test_support::tool_result(&requests[1]);
    assert_eq!(result["tool_use_id"], "call-1");
    assert_eq!(result["content"], "unknown tool: grep");
    assert_eq!(result["is_error"], true);
}

#[tokio::test]
async fn runs_an_mcp_tool_the_model_calls_and_sends_back_its_result() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "mcp__docs__echo", &json!({})),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    agent.mcp.add(crate::mcp::tests::echo_start("docs").await);
    let mut errors = Vec::new();
    let result = agent
        .prompt("echo", &[], |event| {
            if let AgentEvent::ToolDone { name, error, .. } = event {
                errors.push((name, error));
            }
        })
        .await;
    test_support::remove_session(&agent);
    result.unwrap();
    assert_eq!(errors, [("mcp__docs__echo".to_string(), None)]);
    let requests = api.requests().await;
    let result = test_support::tool_result(&requests[1]);
    assert_eq!(result["tool_use_id"], "call-1");
    assert_eq!(result["content"], "pong");
    assert_eq!(result.get("is_error"), Some(&json!(false)));
}
