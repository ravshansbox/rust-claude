use serde_json::json;
use std::time::Duration;

use crate::agent::{
    AgentEvent, cancel_point,
    test_support::{self, MockApi, Reply, text_reply, tool_reply},
};

#[tokio::test]
async fn saves_finished_tool_rounds_while_the_prompt_runs() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "echo round-one" })),
        Reply::Stall,
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let session_file = test_support::session_file(&agent);
    let mut prompt = Box::pin(agent.prompt("run it", &[], |_| {}));
    let second_request_sent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut prompt => return false,
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if api.requests().await.len() == 2 {
                        return true;
                    }
                }
            }
        }
    })
    .await;
    let saved = std::fs::read_to_string(&session_file).unwrap_or_default();
    drop(prompt);
    test_support::remove_session(&agent);
    assert_eq!(second_request_sent, Ok(true));
    assert!(
        saved.contains("tool_result") && saved.contains("round-one"),
        "{saved}"
    );
}

#[tokio::test]
async fn keeps_finished_tool_calls_when_cancelled_during_a_later_call() {
    let two_tool_calls = Reply::Events(vec![
        test_support::message_start(),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "call-1", "name": "bash", "input": { "command": "echo first-done" } } }),
        test_support::block_stop(0),
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "call-2", "name": "bash", "input": { "command": "sleep 30" } } }),
        test_support::block_stop(1),
        test_support::message_delta("tool_use"),
    ]);
    let api = MockApi::start(vec![two_tool_calls, text_reply("ok")]).await;
    let mut agent = test_support::agent(&api);
    let session_file = test_support::session_file(&agent);
    let finished = std::cell::Cell::new(0);
    let mut prompt = Box::pin(agent.prompt("run both", &[], |event| {
        if let AgentEvent::ToolDone { .. } = event {
            finished.set(finished.get() + 1);
        }
    }));
    // The second call sleeps for 30 seconds, so it is still running once
    // the first call is done.
    let first_call_finished = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut prompt => return false,
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if finished.get() == 1 {
                        return true;
                    }
                }
            }
        }
    })
    .await;
    drop(prompt);
    let cancelled = agent.cancel(0);
    let saved = std::fs::read_to_string(&session_file).unwrap_or_default();
    let next = agent.prompt("next", &[], |_| {}).await;
    test_support::remove_session(&agent);
    assert_eq!(first_call_finished, Ok(true));
    assert_eq!(finished.get(), 1);
    cancelled.unwrap();
    next.unwrap();
    assert!(saved.contains("first-done"), "{saved}");
    let requests = api.requests().await;
    let results: Vec<_> = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .map(|block| (block["tool_use_id"].clone(), block["is_error"].clone()))
        .collect();
    assert_eq!(
        results,
        vec![
            (json!("call-1"), json!(false)),
            (json!("call-2"), json!(true))
        ]
    );
    assert!(requests[1]["messages"].to_string().contains("first-done"));
}

#[test]
fn keeps_prompt_and_finished_tool_rounds_on_cancel() {
    let prompt = json!({ "role": "user", "content": "hello" });
    let tool_use = json!({ "role": "assistant", "content": [{ "type": "tool_use" }] });
    let tool_result = json!({ "role": "user", "content": [{ "type": "tool_result" }] });
    assert_eq!(cancel_point(std::slice::from_ref(&prompt), 0), 1);
    let compaction = json!({ "role": "assistant", "stop_reason": "compacted", "content": [] });
    assert_eq!(
        cancel_point(&[compaction.clone(), prompt.clone(), tool_use.clone()], 0),
        2
    );
    assert_eq!(
        cancel_point(&[prompt.clone(), compaction, tool_use.clone()], 0),
        2
    );
    assert_eq!(
        cancel_point(&[prompt, tool_use.clone(), tool_result, tool_use], 0),
        3
    );
}
