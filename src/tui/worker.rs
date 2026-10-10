use crate::{
    agent::{Agent, AgentEvent, ContextUse},
    images::Image,
    mcp::{Added, Started},
    session::SessionSummary,
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
    Models(Result<Vec<String>>),
    ModelChecked(Result<String>),
    ImagePasted(Result<Option<Image>>),
    Shell(String, Result<String>),
    Context(ContextUse),
    Files(u64, Vec<String>),
    Workspace(String),
    McpServer(Added),
    /// The server starts again, after `notice`.
    McpRestarting {
        name: String,
        scope: crate::skills::Scope,
        notice: String,
    },
    Update(crate::update::Progress),
}

pub(super) enum Request {
    Prompt(String, Vec<Image>, &'static str),
    Shell(String),
    Compact(&'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    CheckModel(String),
    NewSession,
    ListModels,
    Context,
    ReplaceMcpServer(Started),
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
                    let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                }
                continue;
            }
            Some(started) = mcp_servers.recv() => {
                let _ = events.send(UiEvent::McpServer(agent.mcp.add(started)));
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
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
            let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
            continue;
        }
        let (prompt, thinking_level) = match request {
            Request::Prompt(prompt, images, thinking_level) => {
                (Some((prompt, images)), thinking_level)
            }
            Request::Compact(thinking_level) => (None, thinking_level),
            Request::Shell(_) => continue,
            Request::Context => {
                let _ = events.send(UiEvent::Context(agent.context_use()));
                continue;
            }
            Request::ReplaceMcpServer(started) => {
                let _ = events.send(UiEvent::McpServer(agent.mcp.replace(started)));
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::ListSessions => {
                let _ = events.send(UiEvent::Sessions(agent.list_sessions()));
                continue;
            }
            Request::Resume(id) => {
                let _ = events.send(UiEvent::Resumed(agent.resume(&id)));
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::SetModel(model) => {
                agent.model = model;
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::NewSession => {
                let _ = events.send(UiEvent::NewSession(agent.new_session()));
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::ListModels => {
                match until_cancelled(&mut cancel, agent.list_models()).await {
                    Some(models) => {
                        let _ = events.send(UiEvent::Models(models));
                    }
                    None => {
                        let _ = events.send(UiEvent::Cancelled(Ok(())));
                    }
                }
                notify_renewed(&mut agent, &events);
                continue;
            }
            Request::CheckModel(model) => {
                let result = match until_cancelled(&mut cancel, agent.list_models()).await {
                    Some(Ok(models)) if models.contains(&model) => {
                        agent.model = model.clone();
                        Ok(model)
                    }
                    Some(Ok(_)) => Err(anyhow::anyhow!("unknown model: {model}")),
                    Some(Err(error)) => Err(error),
                    None => {
                        let _ = events.send(UiEvent::Cancelled(Ok(())));
                        continue;
                    }
                };
                let _ = events.send(UiEvent::ModelChecked(result));
                notify_renewed(&mut agent, &events);
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
        };
        agent.thinking_level = thinking_level;
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
        let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
    }
}

/// Requests the interface lets Esc cancel.
fn is_cancellable(request: &Request) -> bool {
    matches!(
        request,
        Request::Prompt(..)
            | Request::Shell(_)
            | Request::Compact(_)
            | Request::ListModels
            | Request::CheckModel(_)
    )
}

/// Runs `work` unless a cancel arrives or the interface quits first.
async fn until_cancelled<T>(
    cancel: &mut mpsc::UnboundedReceiver<()>,
    work: impl Future<Output = T>,
) -> Option<T> {
    while cancel.try_recv().is_ok() {}
    tokio::select! {
        value = work => Some(value),
        _ = cancel.recv() => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, UiEvent, agent_task, quit};
    use crate::{
        agent::{
            Agent,
            test_support::{self, MockApi, Reply},
        },
        auth::Credentials,
        mcp,
    };
    use serde_json::json;
    use tokio::sync::mpsc;

    fn agent() -> Agent {
        let credentials: Credentials = serde_json::from_value(json!({
            "access": "access",
            "refresh": "refresh",
            "expires": 4_102_444_800_000u64,
        }))
        .unwrap();
        Agent::new(reqwest::Client::new(), credentials, "model".into()).unwrap()
    }

    #[tokio::test]
    async fn does_not_start_requests_sent_before_quitting() {
        let marker = std::env::temp_dir().join(format!("rust-claude-quit-{}", std::process::id()));
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
        if let Some(config_dir) = crate::config::dir() {
            let sessions = config_dir.join("sessions");
            let _ = std::fs::remove_file(sessions.join(format!("{session_id}.jsonl")));
        }
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
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        quit(request_tx, cancel_tx);
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), worker).await;
        if let Some(config_dir) = crate::config::dir() {
            let sessions = config_dir.join("sessions");
            let _ = std::fs::remove_file(sessions.join(format!("{session_id}.jsonl")));
        }
        assert!(stopped.is_ok());
    }

    #[tokio::test]
    async fn stops_loading_models_when_quitting() {
        let api = MockApi::start(vec![Reply::Stall, Reply::Stall]).await;
        let agent = test_support::agent(&api, reqwest::Client::new());
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        request_tx.send(Request::ListModels).unwrap();
        let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        quit(request_tx, cancel_tx);
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), worker).await;
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
        let (agent, _api) = agent_renewing_sign_in().await;
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        quit(request_tx, cancel_tx);
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), worker).await;
        assert!(stopped.is_ok());
    }

    #[tokio::test]
    async fn cancels_a_prompt_sent_while_renewing_the_sign_in() {
        let (agent, _api) = agent_renewing_sign_in().await;
        let session_id = agent.session.id.clone();
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (_mcp_tx, mcp_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
        let session = crate::config::dir()
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
        tokio::time::timeout(std::time::Duration::from_secs(5), wait)
            .await
            .unwrap_or(false)
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
        if let Some(config_dir) = crate::config::dir() {
            let sessions = config_dir.join("sessions");
            let _ = std::fs::remove_file(sessions.join(format!("{session_id}.jsonl")));
        }
        assert!(ran);
        assert!(reported);
    }
}
