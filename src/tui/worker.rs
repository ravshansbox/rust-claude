use crate::{
    agent::{Agent, AgentEvent, ContextUse},
    images::Image,
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
}

pub(super) fn quit(requests: mpsc::UnboundedSender<Request>, cancel: mpsc::UnboundedSender<()>) {
    drop(requests);
    drop(cancel);
}

pub(super) fn notify_renewed(agent: &mut Agent, events: &mpsc::UnboundedSender<UiEvent>) {
    if agent.take_renewed() {
        let _ = events.send(UiEvent::Agent(AgentEvent::Notice(
            "renewed sign-in token".into(),
        )));
    }
}

pub(super) async fn agent_task(
    mut agent: Agent,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut cancel: mpsc::UnboundedReceiver<()>,
    events: mpsc::UnboundedSender<UiEvent>,
) {
    let mut quota = agent.quota_request().await.ok().map(tokio::spawn);
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
            request = requests.recv() => request,
        };
        let Some(request) = request else { break };
        if cancel.is_closed() {
            break;
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
                let _ = events.send(UiEvent::Models(agent.list_models().await));
                notify_renewed(&mut agent, &events);
                continue;
            }
            Request::CheckModel(model) => {
                let result = match agent.list_models().await {
                    Ok(models) if models.contains(&model) => {
                        agent.model = model.clone();
                        Ok(model)
                    }
                    Ok(_) => Err(anyhow::anyhow!("unknown model: {model}")),
                    Err(error) => Err(error),
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

#[cfg(test)]
mod tests {
    use super::{Request, agent_task, quit};
    use crate::{agent::Agent, auth::Credentials};
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
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        request_tx
            .send(Request::Shell(format!("touch {}", marker.display())))
            .unwrap();
        quit(request_tx, cancel_tx);
        agent_task(agent, request_rx, cancel_rx, event_tx).await;
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
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        request_tx.send(Request::Shell("sleep 30".into())).unwrap();
        let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, event_tx));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        quit(request_tx, cancel_tx);
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), worker).await;
        if let Some(config_dir) = crate::config::dir() {
            let sessions = config_dir.join("sessions");
            let _ = std::fs::remove_file(sessions.join(format!("{session_id}.jsonl")));
        }
        assert!(stopped.is_ok());
    }
}
