use super::{Request, UiEvent, agent_task, quit};
use crate::{
    agent::{
        Agent, AgentEvent,
        test_support::{self, MockApi, Reply},
    },
    auth::tests::{AUTH_FILE, answer_token_requests, expired, no_credentials_file},
    config,
    mcp::{self, Started},
    tools::test_support::TemporaryDir,
};
use serde_json::json;
use std::time::Duration;
use tokio::{sync::mpsc, task::JoinHandle};

mod background;
mod quitting;
mod sessions;
mod sign_in;

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

    fn prompt(&self, text: &str, effort: &'static str) {
        self.requests
            .send(Request::Prompt(text.into(), Vec::new(), effort))
            .unwrap();
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
async fn stops_offering_the_tools_of_a_server_replaced_by_a_failed_one() {
    let api = MockApi::start(vec![
        test_support::text_reply("one"),
        test_support::text_reply("two"),
    ])
    .await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker
        .mcp_servers
        .send(mcp::tests::echo_start("docs").await)
        .unwrap();
    let loaded = worker
        .wait_for(|event| matches!(event, UiEvent::McpServer(added) if !added.failed))
        .await;
    worker.prompt("first", "medium");
    let first = worker.wait_for_done().await;
    worker
        .requests
        .send(Request::ReplaceMcpServer(mcp::failed_start("docs", "gone")))
        .unwrap();
    let replaced = worker
        .wait_for(|event| {
            matches!(
                event,
                UiEvent::McpServer(added) if added.status == "MCP server docs failed: gone"
            )
        })
        .await;
    worker.prompt("second", "medium");
    let second = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(loaded && first && replaced && second);
    let requests = api.requests().await;
    let tools = |request: &serde_json::Value| request["tools"].to_string();
    assert!(tools(&requests[0]).contains("mcp__docs__echo"));
    assert!(!tools(&requests[1]).contains("mcp__docs__echo"));
}

#[tokio::test]
async fn cancels_a_running_prompt_and_keeps_it_for_the_next_one() {
    let api = MockApi::start(vec![Reply::Stall, test_support::text_reply("hi")]).await;
    let agent = test_support::agent(&api);
    let session_id = agent.session.id.clone();
    let mut worker = Worker::spawn(agent);
    worker.prompt("first", "off");
    while api.requests().await.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    worker.cancel.send(()).unwrap();
    let cancelled = worker
        .wait_for(|event| matches!(event, UiEvent::Cancelled(Ok(()))))
        .await;
    worker.prompt("second", "off");
    let done = worker.wait_for_done().await;
    let _ = worker.quit().await;
    test_support::remove_session_files(&session_id);
    assert!(cancelled && done);
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let sent = requests[1]["messages"].to_string();
    assert!(sent.contains("first") && sent.contains("second"), "{sent}");
}
