use super::{Request, UiEvent, agent_task, quit};
use crate::{
    agent::{
        Agent, AgentEvent,
        test_support::{self, MockApi, Reply},
    },
    auth::tests::AUTH_FILE,
    config,
    mcp::{self, Started},
};
use serde_json::json;
use std::time::Duration;
use tokio::{sync::mpsc, task::JoinHandle};

fn agent() -> Agent {
    let credentials = test_support::credentials(4_102_444_800_000);
    let mut agent = Agent::new(reqwest::Client::new(), credentials, "model".into()).unwrap();
    agent.api_base = "http://127.0.0.1:1".into();
    agent
}

struct Worker {
    requests: mpsc::UnboundedSender<Request>,
    cancel: mpsc::UnboundedSender<()>,
    mcp_servers: mpsc::UnboundedSender<Started>,
    events: mpsc::UnboundedReceiver<UiEvent>,
    task: JoinHandle<()>,
}

impl Worker {
    fn spawn(agent: Agent) -> Self {
        let (requests, request_rx) = mpsc::unbounded_channel();
        let (cancel, cancel_rx) = mpsc::unbounded_channel();
        let (mcp_servers, mcp_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let task = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
        Self {
            requests,
            cancel,
            mcp_servers,
            events,
            task,
        }
    }

    async fn wait_for(&mut self, mut matches: impl FnMut(&UiEvent) -> bool) -> bool {
        let wait = async {
            while let Some(event) = self.events.recv().await {
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

    async fn wait_for_done(&mut self) -> bool {
        self.wait_for(|event| matches!(event, UiEvent::Done(Ok(()))))
            .await
    }

    fn quit(self) -> JoinHandle<()> {
        quit(self.requests, self.cancel);
        self.task
    }
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
    let worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Shell("sleep 30".into()))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker.quit()).await;
    test_support::remove_session_files(&session_id);
    assert!(stopped.is_ok());
}

#[tokio::test]
async fn quits_while_a_sign_in_still_holds_a_request_sender() {
    let agent = agent();
    let session_id = agent.session.id.clone();
    let worker = Worker::spawn(agent);
    let _signing_in = worker.requests.clone();
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker.quit()).await;
    test_support::remove_session_files(&session_id);
    assert!(stopped.is_ok());
}

#[tokio::test]
async fn shows_the_quota_left_once_it_arrives() {
    let mut agent = agent();
    agent.api_base =
        test_support::quota_server(json!({ "five_hour": { "utilization": 25.0 } })).await;
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    let shown = worker
        .wait_for(|event| {
            matches!(event, UiEvent::Agent(AgentEvent::Stats(stats)) if stats.quota.five_hour_remaining == Some(75.0))
        })
        .await;
    worker.quit().abort();
    test_support::remove_session_files(&session_id);
    assert!(shown);
}

/// An agent whose sign-in has expired and whose renewal never answers.
async fn agent_renewing_sign_in() -> (Agent, MockApi) {
    let api = MockApi::start(vec![Reply::Stall]).await;
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&api.base).unwrap())
        .build()
        .unwrap();
    let agent = Agent::new(http, test_support::credentials(0), "model".into()).unwrap();
    (agent, api)
}

#[tokio::test]
async fn quits_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let worker = Worker::spawn(agent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopped = tokio::time::timeout(Duration::from_secs(5), worker.quit()).await;
    assert!(stopped.is_ok());
}

#[tokio::test]
async fn cancels_a_prompt_sent_while_renewing_the_sign_in() {
    let _lock = AUTH_FILE.lock().await;
    let (agent, _api) = agent_renewing_sign_in().await;
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    worker
        .requests
        .send(Request::Prompt("hello".into(), Vec::new(), "off"))
        .unwrap();
    worker.cancel.send(()).unwrap();
    let cancelled = worker
        .wait_for(|event| matches!(event, UiEvent::Cancelled(Ok(()))))
        .await;
    worker.quit().abort();
    let session = config::dir()
        .map(|dir| dir.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap();
    let saved = std::fs::read_to_string(&session).unwrap_or_default();
    let _ = std::fs::remove_file(&session);
    assert!(cancelled);
    assert!(saved.contains("hello"), "session file: {saved:?}");
}

#[tokio::test]
async fn uses_an_effort_the_new_model_supports_for_background_turns() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "1",
            "bash",
            &json!({ "command": "sleep 0.5", "run_in_background": true }),
        ),
        test_support::text_reply("started"),
        test_support::text_reply("finished"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    agent.model = "claude-haiku-5-5".into();
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Prompt("go".into(), Vec::new(), "off"))
        .unwrap();
    worker
        .requests
        .send(Request::SetModel("claude-opus-5-5".into()))
        .unwrap();
    worker
        .wait_for(|event| matches!(event, UiEvent::Background(_)))
        .await;
    worker
        .wait_for(|event| matches!(event, UiEvent::Done(_)))
        .await;
    worker.quit().abort();
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
            &json!({ "command": "sleep 0.5", "run_in_background": true }),
        ),
        test_support::text_reply("started"),
        test_support::text_reply("finished"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Prompt("go".into(), Vec::new(), "medium"))
        .unwrap();
    worker.requests.send(Request::SetEffort("high")).unwrap();
    worker
        .wait_for(|event| matches!(event, UiEvent::Background(_)))
        .await;
    worker
        .wait_for(|event| matches!(event, UiEvent::Done(_)))
        .await;
    worker.quit().abort();
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
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Shell("echo hi".into()))
        .unwrap();
    let ran = worker
        .wait_for(|event| matches!(event, UiEvent::Shell(_, Ok(output)) if output == "hi\n"))
        .await;
    worker
        .mcp_servers
        .send(mcp::failed_start("slow", "initialize timed out"))
        .unwrap();
    let reported = worker
        .wait_for(|event| {
            matches!(
                event,
                UiEvent::McpServer(added)
                    if added.name == "slow"
                        && added.status == "MCP server slow failed: initialize timed out"
            )
        })
        .await;
    let _ = worker.quit().await;
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
            &json!({ "command": "sleep 0.3; echo finished", "run_in_background": true }),
        ),
        test_support::text_reply("waiting"),
        test_support::text_reply("seen it"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Prompt("watch it".into(), Vec::new(), "off"))
        .unwrap();
    let first = worker.wait_for_done().await;
    let delivered = worker
        .wait_for(|event| matches!(event, UiEvent::Background(text) if text.contains("finished")))
        .await;
    let second = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(first && delivered && second);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    let last = requests[2]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert!(last["content"].to_string().contains("finished"), "{last}");
}

#[tokio::test]
async fn runs_a_prompt_already_sent_before_a_finished_background_command() {
    let api = MockApi::start(vec![
        test_support::tool_reply(
            "call-1",
            "bash",
            &json!({ "command": "echo finished", "run_in_background": true }),
        ),
        test_support::tool_reply("call-2", "bash", &json!({ "command": "sleep 0.3" })),
        test_support::text_reply("waiting"),
        test_support::text_reply("answered"),
        test_support::text_reply("seen it"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    for prompt in ["watch it", "meanwhile"] {
        worker
            .requests
            .send(Request::Prompt(prompt.into(), Vec::new(), "off"))
            .unwrap();
    }
    let delivered = worker
        .wait_for(|event| matches!(event, UiEvent::Background(_)))
        .await;
    let done = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(delivered && done);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 5);
    let last_sent = |request: &serde_json::Value| {
        let messages = request["messages"].as_array().unwrap();
        messages.last().unwrap()["content"].to_string()
    };
    assert!(last_sent(&requests[3]).contains("meanwhile"));
    assert!(last_sent(&requests[4]).contains("finished"));
}

#[tokio::test]
async fn cancels_a_running_prompt_and_keeps_it_for_the_next_one() {
    let api = MockApi::start(vec![Reply::Stall, test_support::text_reply("hi")]).await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Prompt("first".into(), Vec::new(), "off"))
        .unwrap();
    while api.requests().await.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    worker.cancel.send(()).unwrap();
    let cancelled = worker
        .wait_for(|event| matches!(event, UiEvent::Cancelled(Ok(()))))
        .await;
    worker
        .requests
        .send(Request::Prompt("second".into(), Vec::new(), "off"))
        .unwrap();
    let done = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(cancelled && done);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let sent = requests[1]["messages"].to_string();
    assert!(sent.contains("first") && sent.contains("second"), "{sent}");
}

#[tokio::test]
async fn starts_lists_and_resumes_sessions() {
    let api = MockApi::start(vec![
        test_support::text_reply("one"),
        test_support::text_reply("two"),
        test_support::text_reply("three"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let first_id = agent.session.id.clone();
    let second_prompt = format!("after {first_id}");
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Prompt("first".into(), Vec::new(), "off"))
        .unwrap();
    let first = worker.wait_for_done().await;
    worker.requests.send(Request::NewSession).unwrap();
    worker
        .requests
        .send(Request::Prompt(second_prompt.clone(), Vec::new(), "off"))
        .unwrap();
    let second = worker.wait_for_done().await;
    worker.requests.send(Request::ListSessions).unwrap();
    let listed = worker
        .wait_for(|event| {
            matches!(event, UiEvent::Sessions(Ok(sessions))
                if sessions.iter().any(|session| session.id == first_id))
        })
        .await;
    worker
        .requests
        .send(Request::Resume(first_id.clone()))
        .unwrap();
    let resumed = worker
        .wait_for(|event| {
            matches!(event, UiEvent::Resumed(Ok(messages))
                if serde_json::to_string(messages).unwrap().contains("first"))
        })
        .await;
    worker.requests.send(Request::ListSessions).unwrap();
    let mut second_id = None;
    worker
        .wait_for(|event| {
            let UiEvent::Sessions(Ok(sessions)) = event else {
                return false;
            };
            second_id = sessions
                .iter()
                .find(|session| session.preview.contains(&second_prompt))
                .map(|session| session.id.clone());
            true
        })
        .await;
    worker
        .requests
        .send(Request::Prompt("third".into(), Vec::new(), "off"))
        .unwrap();
    let third = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&first_id);
    if let Some(id) = &second_id {
        test_support::remove_session_files(id);
    }
    assert!(first && second && listed && resumed && third);
    assert!(second_id.is_some());
    let sent: Vec<String> = api
        .requests()
        .await
        .iter()
        .map(|request| request["messages"].to_string())
        .collect();
    assert!(!sent[1].contains("first"), "{}", sent[1]);
    assert!(sent[2].contains("first") && sent[2].contains("third"));
    assert!(!sent[2].contains(&second_prompt), "{}", sent[2]);
}

#[tokio::test]
async fn compacts_the_conversation_and_continues_from_the_summary() {
    let api = MockApi::start(vec![
        test_support::text_reply("one"),
        test_support::text_reply("the summary"),
        test_support::text_reply("two"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker
        .requests
        .send(Request::Prompt("first".into(), Vec::new(), "medium"))
        .unwrap();
    let first = worker.wait_for_done().await;
    worker.requests.send(Request::Compact("high")).unwrap();
    let compacted = worker.wait_for_done().await;
    worker
        .requests
        .send(Request::Prompt("second".into(), Vec::new(), "medium"))
        .unwrap();
    let second = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(first && compacted && second);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["output_config"]["effort"], "high");
    let sent = requests[2]["messages"].to_string();
    assert!(
        sent.contains("the summary") && sent.contains("second"),
        "{sent}"
    );
    assert!(!sent.contains("first"), "{sent}");
}
