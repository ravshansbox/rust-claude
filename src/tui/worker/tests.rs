use super::{Request, UiEvent, agent_task, quit};
use crate::{
    agent::{
        Agent,
        test_support::{self, MockApi, Reply},
    },
    auth::{Credentials, tests::AUTH_FILE},
    config, mcp,
};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

fn agent() -> Agent {
    let credentials: Credentials = serde_json::from_value(json!({
        "access": "access",
        "refresh": "refresh",
        "expires": 4_102_444_800_000u64,
    }))
    .unwrap();
    let mut agent = Agent::new(reqwest::Client::new(), credentials, "model".into()).unwrap();
    agent.api_base = "http://127.0.0.1:1".into();
    agent
}

#[tokio::test]
async fn does_not_start_requests_sent_before_quitting() {
    let marker =
        std::env::temp_dir().join(format!("rust-coding-agent-quit-{}", std::process::id()));
    let agent = agent();
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    request_tx
        .send(Request::Shell(format!("touch {}", marker.display())))
        .unwrap();
    quit(request_tx, cancel_tx);
    agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx).await;
    let ran = marker.exists();
    let _ = std::fs::remove_file(&marker);
    test_support::remove_session_files(&session_id);
    assert!(!ran);
}

#[tokio::test]
async fn stops_running_request_when_quitting() {
    let agent = agent();
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    request_tx.send(Request::Shell("sleep 30".into())).unwrap();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    tokio::time::sleep(Duration::from_millis(200)).await;
    quit(request_tx, cancel_tx);
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker).await;
    test_support::remove_session_files(&session_id);
    assert!(stopped.is_ok());
}

/// An agent whose sign-in has expired and whose renewal never answers.
async fn agent_renewing_sign_in() -> (Agent, MockApi) {
    let api = MockApi::start(vec![Reply::Stall]).await;
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&api.base).unwrap())
        .build()
        .unwrap();
    let credentials: Credentials = serde_json::from_value(json!({
        "access": "access",
        "refresh": "refresh",
        "expires": 0,
    }))
    .unwrap();
    let agent = Agent::new(http, credentials, "model".into()).unwrap();
    (agent, api)
}

#[tokio::test]
async fn quits_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    tokio::time::sleep(Duration::from_millis(200)).await;
    quit(request_tx, cancel_tx);
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker).await;
    assert!(stopped.is_ok());
}

#[tokio::test]
async fn cancels_a_prompt_sent_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    tokio::time::sleep(Duration::from_millis(200)).await;
    request_tx
        .send(Request::Prompt("hello".into(), Vec::new(), "off"))
        .unwrap();
    cancel_tx.send(()).unwrap();
    let cancelled = wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Cancelled(Ok(())))
    })
    .await;
    quit(request_tx, cancel_tx);
    worker.abort();
    let session = config::dir()
        .map(|dir| dir.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap();
    let saved = std::fs::read_to_string(&session).unwrap_or_default();
    let _ = std::fs::remove_file(&session);
    assert!(cancelled);
    assert!(saved.contains("hello"), "session file: {saved:?}");
}

async fn wait_for_event(
    events: &mut mpsc::UnboundedReceiver<UiEvent>,
    matches: impl Fn(&UiEvent) -> bool,
) -> bool {
    let wait = async {
        while let Some(event) = events.recv().await {
            if matches(&event) {
                return true;
            }
        }
        false
    };
    tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .unwrap_or(false)
}

#[tokio::test]
async fn uses_an_effort_the_new_model_supports_for_background_turns() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "1",
            "bash",
            json!({ "command": "sleep 0.5", "run_in_background": true }),
        ),
        test_support::text_reply("started"),
        test_support::text_reply("finished"),
    ])
    .await;
    let mut agent = test_support::agent(&api, reqwest::Client::new());
    agent.model = "claude-haiku-5-5".into();
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    request_tx
        .send(Request::Prompt("go".into(), Vec::new(), "off"))
        .unwrap();
    request_tx
        .send(Request::SetModel("claude-opus-5-5".into()))
        .unwrap();
    wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Background(_))
    })
    .await;
    wait_for_event(&mut event_rx, |event| matches!(event, UiEvent::Done(_))).await;
    quit(request_tx, cancel_tx);
    worker.abort();
    test_support::remove_session_files(&session_id);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2]["model"], "claude-opus-5-5");
    assert_eq!(requests[2]["output_config"]["effort"], "medium");
}

#[tokio::test]
async fn uses_the_effort_chosen_since_the_last_prompt_for_background_turns() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "1",
            "bash",
            json!({ "command": "sleep 0.5", "run_in_background": true }),
        ),
        test_support::text_reply("started"),
        test_support::text_reply("finished"),
    ])
    .await;
    let agent = test_support::agent(&api, reqwest::Client::new());
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    request_tx
        .send(Request::Prompt("go".into(), Vec::new(), "medium"))
        .unwrap();
    request_tx.send(Request::SetEffort("high")).unwrap();
    wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Background(_))
    })
    .await;
    wait_for_event(&mut event_rx, |event| matches!(event, UiEvent::Done(_))).await;
    quit(request_tx, cancel_tx);
    worker.abort();
    test_support::remove_session_files(&session_id);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["output_config"]["effort"], "medium");
    assert_eq!(requests[2]["output_config"]["effort"], "high");
}

#[tokio::test]
async fn runs_requests_while_mcp_servers_start() {
    let agent = agent();
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    request_tx.send(Request::Shell("echo hi".into())).unwrap();
    let ran = wait_for_event(
        &mut event_rx,
        |event| matches!(event, UiEvent::Shell(_, Ok(output)) if output == "hi\n"),
    )
    .await;
    mcp_tx
        .send(mcp::failed_start("slow", "initialize timed out"))
        .unwrap();
    let reported = wait_for_event(&mut event_rx, |event| {
        matches!(
            event,
            UiEvent::McpServer(added)
                if added.name == "slow"
                    && added.status == "MCP server slow failed: initialize timed out"
        )
    })
    .await;
    quit(request_tx, cancel_tx);
    let _ = worker.await;
    test_support::remove_session_files(&session_id);
    assert!(ran);
    assert!(reported);
}

#[tokio::test]
async fn starts_a_turn_when_a_background_command_finishes() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "call-1",
            "bash",
            json!({ "command": "sleep 0.3; echo finished", "run_in_background": true }),
        ),
        test_support::text_reply("waiting"),
        test_support::text_reply("seen it"),
    ])
    .await;
    let agent = test_support::agent(&api, reqwest::Client::new());
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    request_tx
        .send(Request::Prompt("watch it".into(), Vec::new(), "off"))
        .unwrap();
    let first = wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Done(Ok(())))
    })
    .await;
    let delivered = wait_for_event(
        &mut event_rx,
        |event| matches!(event, UiEvent::Background(text) if text.contains("finished")),
    )
    .await;
    let second = wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Done(Ok(())))
    })
    .await;
    quit(request_tx, cancel_tx);
    let _ = worker.await;
    test_support::remove_session_files(&session_id);
    assert!(first && delivered && second);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let last = requests[2]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert!(last["content"].to_string().contains("finished"), "{last}");
}

#[tokio::test]
async fn cancels_a_running_prompt_and_keeps_it_for_the_next_one() {
    let api = MockApi::start(vec![Reply::Stall, test_support::text_reply("hi")]).await;
    let agent = test_support::agent(&api, reqwest::Client::new());
    let session_id = agent.session.id.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    request_tx
        .send(Request::Prompt("first".into(), Vec::new(), "off"))
        .unwrap();
    while api.requests().await.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancel_tx.send(()).unwrap();
    let cancelled = wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Cancelled(Ok(())))
    })
    .await;
    request_tx
        .send(Request::Prompt("second".into(), Vec::new(), "off"))
        .unwrap();
    let done = wait_for_event(&mut event_rx, |event| {
        matches!(event, UiEvent::Done(Ok(())))
    })
    .await;
    quit(request_tx, cancel_tx);
    let _ = worker.await;
    test_support::remove_session_files(&session_id);
    assert!(cancelled && done);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let sent = requests[1]["messages"].to_string();
    assert!(sent.contains("first") && sent.contains("second"), "{sent}");
}
