use crate::{
    agent::{Agent, AgentEvent, DEFAULT_EFFORT},
    images::Image,
    mcp::{Added, Started},
    models,
    session::SessionSummary,
    skills::Scope,
    update::Progress,
};
use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

pub(super) enum UiEvent {
    Agent(AgentEvent),
    Done(Result<()>),
    Cancelled(Result<()>),
    Sessions(Result<Vec<SessionSummary>>),
    Resumed(Result<Vec<Value>>),
    NewSession(Result<()>),
    ImagePasted(Result<Option<Image>>),
    Shell(String, Result<String>),
    Files(u64, Vec<String>),
    Workspace(String),
    McpServer(Added),
    /// The server starts again, after `notice`.
    McpRestarting {
        name: String,
        scope: Scope,
        notice: String,
    },
    Update(Progress),
    /// A background command finished, and a turn starts with this message.
    Background(String),
}

pub(super) enum Request {
    Prompt(String, Vec<Image>, &'static str),
    Shell(String),
    Compact(&'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    SetEffort(&'static str),
    NewSession,
    ReplaceMcpServer(Started),
    /// Starts a turn with the message of a finished background command.
    Background(String),
}

pub(super) fn quit(requests: mpsc::UnboundedSender<Request>, cancel: mpsc::UnboundedSender<()>) {
    drop(requests);
    drop(cancel);
}

pub(super) fn notify_renewed(agent: &mut Agent, events: &mpsc::UnboundedSender<UiEvent>) {
    if let Some(notice) = agent.take_renewal_notice() {
        let _ = events.send(UiEvent::Agent(AgentEvent::Notice(notice)));
    }
}

fn send_stats(agent: &Agent, events: &mpsc::UnboundedSender<UiEvent>) {
    let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
}

pub(super) async fn agent_task(
    mut agent: Agent,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut cancel: mpsc::UnboundedReceiver<()>,
    mut mcp_servers: mpsc::UnboundedReceiver<Started>,
    events: mpsc::UnboundedSender<UiEvent>,
) {
    // Stop renewing the sign-in on quit or Esc. Esc means the user cancelled
    // the request waiting behind the renewal, so cancel it when it comes up.
    let mut cancelled = false;
    let mut quota = tokio::select! {
        result = agent.quota_request() => result.ok().map(tokio::spawn),
        received = cancel.recv() => {
            cancelled = received.is_some();
            None
        }
    };
    notify_renewed(&mut agent, &events);
    loop {
        let request = tokio::select! {
            Some(result) = async { Some(quota.as_mut()?.await) }, if quota.is_some() => {
                quota = None;
                if let Ok(Ok(value)) = result {
                    agent.merge_quota(value);
                    send_stats(&agent, &events);
                }
                continue;
            }
            Some(started) = mcp_servers.recv() => {
                let _ = events.send(UiEvent::McpServer(agent.mcp.add(started)));
                send_stats(&agent, &events);
                continue;
            }
            Some(message) = agent.next_background(), if agent.has_background() => {
                Some(Request::Background(message))
            }
            request = requests.recv() => request,
        };
        let Some(request) = request else { break };
        if cancel.is_closed() {
            break;
        }
        if cancelled && is_cancellable(&request) {
            cancelled = false;
            let result = match &request {
                Request::Prompt(prompt, images, _) => agent.cancel_unsent(prompt, images),
                _ => Ok(()),
            };
            let _ = events.send(UiEvent::Cancelled(result));
            continue;
        }
        if let Request::Shell(command) = request {
            while cancel.try_recv().is_ok() {}
            tokio::select! {
                result = agent.shell(&command) => {
                    let _ = events.send(UiEvent::Shell(command, result));
                }
                _ = cancel.recv() => {
                    let _ = events.send(UiEvent::Cancelled(Ok(())));
                }
            }
            send_stats(&agent, &events);
            continue;
        }
        let (prompt, effort) = match request {
            Request::Prompt(prompt, images, effort) => (Some((prompt, images)), effort),
            Request::Background(message) => {
                let _ = events.send(UiEvent::Background(message.clone()));
                (Some((message, Vec::new())), agent.effort)
            }
            Request::Compact(effort) => (None, effort),
            Request::Shell(_) => continue,
            Request::ReplaceMcpServer(started) => {
                let _ = events.send(UiEvent::McpServer(agent.mcp.replace(started)));
                send_stats(&agent, &events);
                continue;
            }
            Request::ListSessions => {
                let _ = events.send(UiEvent::Sessions(agent.list_sessions().await));
                continue;
            }
            Request::Resume(id) => {
                let _ = events.send(UiEvent::Resumed(agent.resume(&id).await));
                send_stats(&agent, &events);
                continue;
            }
            Request::SetModel(model) => {
                if !models::effort_levels(&model).contains(&agent.effort) {
                    agent.effort = DEFAULT_EFFORT;
                }
                agent.model = model;
                send_stats(&agent, &events);
                continue;
            }
            Request::SetEffort(effort) => {
                agent.effort = effort;
                continue;
            }
            Request::NewSession => {
                let _ = events.send(UiEvent::NewSession(agent.new_session()));
                send_stats(&agent, &events);
                continue;
            }
        };
        agent.effort = effort;
        while cancel.try_recv().is_ok() {}

        let checkpoint = agent.history_len();
        let cancelled = {
            let on_event = |event| {
                let _ = events.send(UiEvent::Agent(event));
            };
            let run = async {
                match &prompt {
                    Some((prompt, images)) => agent.prompt(prompt, images, on_event).await,
                    None => agent.compact(on_event).await,
                }
            };
            tokio::select! {
                result = run => {
                    let _ = events.send(UiEvent::Done(result));
                    false
                }
                _ = cancel.recv() => true,
            }
        };
        if cancelled {
            let _ = events.send(UiEvent::Cancelled(agent.cancel(checkpoint)));
        }
        send_stats(&agent, &events);
    }
}

/// Requests the interface lets Esc cancel.
fn is_cancellable(request: &Request) -> bool {
    matches!(
        request,
        Request::Prompt(..) | Request::Shell(_) | Request::Compact(_)
    )
}

#[cfg(test)]
mod tests {
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
}
