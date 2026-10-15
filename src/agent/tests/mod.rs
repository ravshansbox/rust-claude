use serde_json::{Value, json};

use super::{
    AgentEvent,
    test_support::{self, MockApi, Reply, text_reply, tool_reply},
};
use crate::images::Image;

mod queue;
mod shell;
mod thinking;
mod tool_rounds;
mod web_fetch;
mod web_search;

#[tokio::test]
async fn reports_why_a_prompt_failed_when_saving_the_session_also_fails() {
    let api = MockApi::start(vec![Reply::BadRequest("prompt is too long".into())]).await;
    let mut agent = test_support::agent(&api);
    // A session file in a folder that does not exist cannot be written.
    agent.session.id = format!("{}/missing", agent.session.id);
    let error = agent.prompt("hi", &[], |_| {}).await.unwrap_err();
    let message = error.to_string();
    assert!(message.contains("prompt is too long"), "{message}");
    assert!(message.contains("failed to save session"), "{message}");
}

#[tokio::test]
async fn shows_only_context_counts_the_api_reported() {
    let api = MockApi::start(vec![
        tool_reply(
            "call-1",
            "bash",
            &json!({ "command": "head -c 8000 /dev/zero | tr '\\0' x" }),
        ),
        text_reply("done"),
        text_reply("summary of the work"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let before = agent.stats().context_tokens;
    let mut counts = Vec::new();
    let result = agent
        .prompt("run it", &[], |event| {
            if let AgentEvent::Stats(stats) = event {
                counts.push(stats.context_tokens);
            }
        })
        .await;
    let compacted = agent.compact(|_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    compacted.unwrap();
    assert_eq!(before, None);
    counts.dedup();
    assert_eq!(counts, [None, Some(2)]);
    assert_eq!(agent.stats().context_tokens, None);
}

#[tokio::test]
async fn keeps_the_conversation_cached_after_a_round_with_many_tool_calls() {
    let mut events = vec![
        test_support::message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "plan", "signature": "sig" } }),
        test_support::block_stop(0),
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "running" } }),
        test_support::block_stop(1),
    ];
    for index in 2..12 {
        events.push(json!({ "type": "content_block_start", "index": index, "content_block": { "type": "tool_use", "id": format!("call-{index}"), "name": "bash", "input": { "command": "true" } } }));
        events.push(test_support::block_stop(index));
    }
    events.push(test_support::message_delta("tool_use"));
    let api = MockApi::start(vec![Reply::Events(events), text_reply("done")]).await;
    let (_, result) = test_support::run_prompt(&api, "run them all", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let body = &requests[1];
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    let breakpoint = |message: &Value| match message["content"].as_array() {
        Some(blocks) => blocks.last().unwrap()["cache_control"].clone(),
        None => Value::Null,
    };
    let ephemeral = json!({ "type": "ephemeral" });
    // The prompt ended the first request, where its cache entry was written.
    assert_eq!(breakpoint(&messages[0]), ephemeral);
    assert_eq!(breakpoint(&messages[2]), ephemeral);
    assert!(
        body.to_string().matches("cache_control").count() <= 4,
        "{body}"
    );
}

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

#[tokio::test]
async fn reads_the_quota_left_and_keeps_the_figures_replies_reported() {
    let base = test_support::quota_server(json!({
        "five_hour": { "utilization": 25.0, "resets_at": "1970-01-01T01:00:00Z" },
        "seven_day": { "utilization": 90.0, "resets_at": "1970-01-08T00:00:00+00:00" },
    }))
    .await;
    let api = MockApi::start(Vec::new()).await;
    let mut agent = test_support::agent(&api);
    agent.api_base = base;
    agent.quota.five_hour_remaining = Some(40.0);
    let quota = agent.quota_request().await.unwrap().await.unwrap();
    agent.merge_quota(quota);
    test_support::remove_session(&agent);
    let quota = agent.stats().quota;
    assert_eq!(quota.five_hour_remaining, Some(40.0));
    assert_eq!(quota.five_hour_reset, Some(3_600));
    assert_eq!(quota.seven_day_remaining, Some(10.0));
    assert_eq!(quota.seven_day_reset, Some(604_800));
}

#[tokio::test]
async fn reports_a_skill_whose_file_was_removed_without_sending_the_prompt() {
    use crate::skills::{Scope, Skill};
    let directory = crate::tools::test_support::TemporaryDir::new("agent-removed-skill");
    let path = directory.path().join("SKILL.md");
    let api = MockApi::start(vec![text_reply("hello")]).await;
    let mut agent = test_support::agent(&api);
    agent.skills.skills = vec![Skill {
        name: "demo".into(),
        description: "Demo.".into(),
        path: path.clone(),
        base_dir: directory.path().into(),
        disable_model_invocation: false,
        scope: Scope::Project,
    }];
    let result = agent.prompt("/skill:demo fix it", &[], |_| {}).await;
    test_support::remove_session(&agent);
    let error = result.unwrap_err().to_string();
    assert_eq!(error, format!("failed to read skill {}", path.display()));
    assert_eq!(agent.messages(), Vec::<Value>::new());
    assert_eq!(api.requests().await, Vec::<Value>::new());
}

#[tokio::test]
async fn sends_pasted_images_before_the_prompt() {
    let api = MockApi::start(vec![text_reply("a red square")]).await;
    let mut agent = test_support::agent(&api);
    let image = Image {
        media_type: "image/png",
        data: b"png".to_vec(),
    };
    let result = agent.prompt("what is this?", &[image], |_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(
        requests[0]["messages"][0]["content"],
        json!([
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "cG5n" } },
            { "type": "text", "text": "what is this?", "cache_control": { "type": "ephemeral" } },
        ])
    );
}
