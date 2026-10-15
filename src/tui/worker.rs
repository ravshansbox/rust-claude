use crate::{
    agent::{Agent, AgentEvent},
    images::Image,
    mcp::{Added, Started},
    session::SessionSummary,
    skills::Scope,
    update::Progress,
};
use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

mod handle;

use handle::{handle_request, run_turn};

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
        send_notice(events, notice);
    }
}

/// Shows `text` as a notice.
pub(super) fn send_notice(events: &mpsc::UnboundedSender<UiEvent>, text: String) {
    let _ = events.send(UiEvent::Agent(AgentEvent::Notice(text)));
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
        // Requests the interface already sent come ahead of a background turn.
        let request = tokio::select! {
            biased;
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
            // Quitting closes `cancel`; a sign-in to an MCP server may still
            // hold a sender of `requests` open for minutes.
            None = cancel.recv() => None,
            request = requests.recv() => request,
            Some(message) = agent.background.next(), if !agent.background.is_empty() => {
                Some(Request::Background(message))
            }
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
        let Some((prompt, effort)) =
            handle_request(&mut agent, request, &mut cancel, &events).await
        else {
            continue;
        };
        run_turn(&mut agent, prompt, effort, &mut cancel, &events).await;
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
mod tests;
