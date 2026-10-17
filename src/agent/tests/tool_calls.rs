use serde_json::json;

use crate::agent::{
    AgentEvent,
    test_support::{self, MockApi, text_reply, tool_reply},
};

#[tokio::test]
async fn tells_the_model_a_tool_it_named_does_not_exist() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "find_files", &json!({ "pattern": "fn" })),
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
        [(
            "find_files".to_string(),
            Some("unknown tool: find_files".to_string())
        )]
    );
    let requests = api.requests().await;
    let result = test_support::tool_result(&requests[1]);
    assert_eq!(result["tool_use_id"], "call-1");
    assert_eq!(result["content"], "unknown tool: find_files");
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
async fn sends_an_image_the_model_reads_scaled_to_fit_2000_pixels() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use image::{DynamicImage, GenericImageView, ImageFormat, RgbImage};
    let file = crate::tools::test_support::TemporaryFile::new("read-image", "");
    DynamicImage::ImageRgb8(RgbImage::new(4000, 1000))
        .save_with_format(file.path(), ImageFormat::Png)
        .unwrap();
    let api = MockApi::start(vec![
        tool_reply("call-1", "read", &json!({ "path": file.path() })),
        text_reply("a black strip"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let result = agent.prompt("look", &[], |_| {}).await;
    let saved = std::fs::read_to_string(test_support::session_file(&agent)).unwrap();
    test_support::remove_session(&agent);
    result.unwrap();
    let requests = api.requests().await;
    let result = test_support::tool_result(&requests[1]);
    assert_eq!(result["is_error"], false);
    let image = &result["content"][0];
    assert_eq!(image["type"], "image");
    assert_eq!(image["source"]["media_type"], "image/png");
    let data = STANDARD
        .decode(image["source"]["data"].as_str().unwrap())
        .unwrap();
    let decoded = image::load_from_memory(&data).unwrap();
    assert_eq!(decoded.dimensions(), (2000, 500));
    assert!(!saved.contains(image["source"]["data"].as_str().unwrap()));
}

#[tokio::test]
async fn offers_ast_grep_and_grep_and_says_which_to_prefer() {
    let api = MockApi::start(vec![text_reply("hello")]).await;
    let (_, result) = test_support::run_prompt(&api, "hi", |_| {}).await;
    result.unwrap();
    let requests = api.requests().await;
    let tools = requests[0]["tools"].as_array().unwrap();
    assert!(
        ["ast_grep", "grep"]
            .iter()
            .all(|name| tools.iter().any(|tool| tool["name"] == *name)),
        "{tools:?}"
    );
    let system = requests[0]["system"].to_string();
    assert!(
        system.contains("To find code, use ast_grep first, then grep, then read or bash."),
        "{system}"
    );
}
