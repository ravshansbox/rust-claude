use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

use super::{
    AgentEvent, Queued, cancel_point,
    test_support::{self, MockApi, Reply, text_reply, tool_reply},
};
use crate::{
    images::Image,
    tools::test_support::{TemporaryFile, is_running},
};

mod web;

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
async fn drops_thinking_bound_to_a_changed_conversation_for_the_rest_of_the_session() {
    let mismatch = "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation. Remove the block, or set `thinking.block_binding.prefix_mismatch_behavior` to \"drop_block\". The `tools` list differs from when the block was created.";
    let api = MockApi::start(vec![
        text_reply("first"),
        Reply::BadRequest(mismatch.into()),
        text_reply("second"),
        text_reply("third"),
        text_reply("after restart"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let mut notices = Vec::new();
    let first = agent.prompt("one", &[], |_| {}).await;
    let second = agent
        .prompt("two", &[], test_support::notices(&mut notices))
        .await;
    let third = agent.prompt("three", &[], |_| {}).await;
    // Close the session, as quitting does, so that it can be resumed.
    let id = agent.session.id.clone();
    drop(agent);
    let mut restarted = test_support::agent(&api);
    let resumed = restarted.resume(&id).await;
    let after_restart = restarted.prompt("four", &[], |_| {}).await;
    test_support::remove_session(&restarted);
    first.unwrap();
    second.unwrap();
    third.unwrap();
    resumed.unwrap();
    after_restart.unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    let requests = api.requests().await;
    let headers = api.headers().await;
    assert_eq!(requests.len(), 5);
    let drop_block = json!({ "prefix_mismatch_behavior": "drop_block" });
    for index in 0..2 {
        assert_eq!(requests[index]["thinking"].get("block_binding"), None);
        assert!(!headers[index].contains("thinking-binding-controls-2026-08-01"));
    }
    for index in 2..5 {
        assert_eq!(requests[index]["thinking"]["block_binding"], drop_block);
        assert!(
            headers[index]
                .contains("anthropic-beta: oauth-2025-04-20,thinking-binding-controls-2026-08-01"),
            "{}",
            headers[index]
        );
    }
}

#[tokio::test]
async fn turns_thinking_off_without_asking_to_drop_mismatched_thinking() {
    for (model, kind) in [
        ("claude-sonnet-5-5", "between_tools"),
        ("claude-haiku-5-5", "disabled"),
    ] {
        let api = MockApi::start(vec![text_reply("done")]).await;
        let mut agent = test_support::agent(&api);
        agent.model = model.into();
        agent.effort = "off";
        agent.session.drop_mismatched_thinking();
        let result = agent.prompt("hello", &[], |_| {}).await;
        test_support::remove_session(&agent);
        result.unwrap();
        let requests = api.requests().await;
        let headers = api.headers().await;
        assert_eq!(requests[0]["thinking"], json!({ "type": kind }), "{model}");
        assert_eq!(requests[0].get("output_config"), None, "{model}");
        assert!(
            headers[0].contains("anthropic-beta: oauth-2025-04-20\r\n"),
            "{}",
            headers[0]
        );
    }
}

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
async fn lets_prompts_be_queued_while_queued_images_are_saved() {
    use std::sync::{
        TryLockError,
        atomic::{AtomicBool, Ordering},
    };

    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "true" })),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let small = Image {
        media_type: "image/png",
        data: vec![1; 16],
    };
    // Large enough that saving it takes a while.
    let large = Image {
        media_type: "image/png",
        data: vec![2; 8 << 20],
    };
    agent.queue.lock().unwrap().push(Queued {
        prompt: "look at these".into(),
        images: vec![(1, small), (2, large)],
    });
    let directory = test_support::session_file(&agent).with_file_name(&agent.session.id);
    let queue = agent.queue.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let watching = finished.clone();
    // Watches the queue while the first image is saved and the second
    // one is being saved.
    let watcher = std::thread::spawn(move || {
        let mut blocked = false;
        while !watching.load(Ordering::Relaxed) {
            let saved = std::fs::read_dir(&directory).map_or(0, |entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension() == Some("png".as_ref()))
                    .count()
            });
            if saved == 1 && matches!(queue.try_lock(), Err(TryLockError::WouldBlock)) {
                blocked = true;
            }
        }
        blocked
    });
    let result = agent.prompt("run it", &[], |_| {}).await;
    finished.store(true, Ordering::Relaxed);
    let blocked = watcher.join().unwrap();
    test_support::remove_session(&agent);
    result.unwrap();
    assert!(!blocked, "the queue was locked while an image was saved");
    assert!(agent.queue.lock().unwrap().is_empty());
    let requests = api.requests().await;
    assert!(
        requests[1]["messages"]
            .to_string()
            .contains("look at these")
    );
}

#[tokio::test]
async fn keeps_prompts_queued_when_their_image_cannot_be_saved() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "true" })),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let image = Image {
        media_type: "image/png",
        data: vec![1; 16],
    };
    agent.queue.lock().unwrap().push(Queued {
        prompt: "look at this".into(),
        images: vec![(1, image)],
    });
    // A file in the way of the session's image folder makes saving fail.
    let images = test_support::session_file(&agent).with_file_name(&agent.session.id);
    std::fs::create_dir_all(images.parent().unwrap()).unwrap();
    std::fs::write(&images, "").unwrap();
    let result = agent.prompt("run it", &[], |_| {}).await;
    let _ = std::fs::remove_file(&images);
    test_support::remove_session(&agent);
    assert!(result.is_err());
    let queued: Vec<_> = agent
        .queue
        .lock()
        .unwrap()
        .iter()
        .map(|queued| queued.prompt.clone())
        .collect();
    assert_eq!(queued, ["look at this"]);
}

#[tokio::test]
async fn keeps_the_conversation_cached_when_a_queued_prompt_follows_a_tool_round() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "true" })),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    agent.queue.lock().unwrap().push(Queued {
        prompt: "also this".into(),
        images: Vec::new(),
    });
    let result = agent.prompt("run it", &[], |_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    let marked: Vec<bool> = messages
        .iter()
        .map(|message| message.to_string().contains("cache_control"))
        .collect();
    // The prompt ended the first request, where its cache entry was written.
    assert_eq!(marked, [true, false, false, true]);
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
