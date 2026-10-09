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
