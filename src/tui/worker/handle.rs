use super::{Request, UiEvent, send_stats};
use crate::{agent::Agent, images::Image, models};
use tokio::sync::mpsc;

/// Handles `request`, and returns the prompt and effort of a turn it starts.
pub(super) async fn handle_request(
    agent: &mut Agent,
    request: Request,
    cancel: &mut mpsc::UnboundedReceiver<()>,
    events: &mpsc::UnboundedSender<UiEvent>,
) -> Option<(Option<(String, Vec<Image>)>, &'static str)> {
    match request {
        Request::Prompt(prompt, images, effort) => return Some((Some((prompt, images)), effort)),
        Request::Background(message) => {
            let _ = events.send(UiEvent::Background(message.clone()));
            return Some((Some((message, Vec::new())), agent.effort));
        }
        Request::Compact(effort) => return Some((None, effort)),
        Request::Shell(command) => {
            drain(cancel);
            tokio::select! {
                result = agent.shell(&command) => {
                    let _ = events.send(UiEvent::Shell(command, result));
                }
                _ = cancel.recv() => {
                    let _ = events.send(UiEvent::Cancelled(Ok(())));
                }
            }
        }
        Request::ReplaceMcpServer(started) => {
            let _ = events.send(UiEvent::McpServer(agent.mcp.replace(started)));
        }
        Request::ListSessions => {
            let _ = events.send(UiEvent::Sessions(agent.list_sessions().await));
            return None;
        }
        Request::Resume(id) => {
            let _ = events.send(UiEvent::Resumed(agent.resume(&id).await));
        }
        Request::SetModel(model) => {
            agent.effort = models::supported_effort(&model, agent.effort);
            agent.model = model;
        }
        Request::SetEffort(effort) => {
            agent.effort = effort;
            return None;
        }
        Request::NewSession => {
            let _ = events.send(UiEvent::NewSession(agent.new_session()));
        }
    }
    send_stats(agent, events);
    None
}

pub(super) async fn run_turn(
    agent: &mut Agent,
    prompt: Option<(String, Vec<Image>)>,
    effort: &'static str,
    cancel: &mut mpsc::UnboundedReceiver<()>,
    events: &mpsc::UnboundedSender<UiEvent>,
) {
    agent.effort = effort;
    drain(cancel);

    let checkpoint = agent.messages().len();
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
    send_stats(agent, events);
}

/// Drops cancels sent before the work they could cancel started.
fn drain(cancel: &mut mpsc::UnboundedReceiver<()>) {
    while cancel.try_recv().is_ok() {}
}
