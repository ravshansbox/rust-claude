use super::{
    keys::Action,
    mcp_sign_in::{sign_in_to_mcp_server, sign_out_of_mcp_server},
    worker::{Request, UiEvent},
};
use crate::{clipboard, images};
use tokio::sync::mpsc;

pub(super) fn perform(
    action: Action,
    requests: &mpsc::UnboundedSender<Request>,
    cancel: &mpsc::UnboundedSender<()>,
    events: &mpsc::UnboundedSender<UiEvent>,
) {
    let send = |request| {
        let _ = requests.send(request);
    };
    match action {
        Action::Submit(prompt, images, effort) => send(Request::Prompt(prompt, images, effort)),
        Action::SubmitLoop(prompt, images, effort) => {
            send(Request::LoopPrompt(prompt, images, effort));
        }
        Action::Shell(command, share) => send(Request::Shell(command, share)),
        Action::PasteImage => {
            let events = events.clone();
            tokio::spawn(async move {
                let result = clipboard::read_image()
                    .await
                    .and_then(|data| data.map(images::prepare).transpose());
                let _ = events.send(UiEvent::ImagePasted(result));
            });
        }
        Action::Compact(effort) => send(Request::Compact(effort)),
        Action::ListSessions => send(Request::ListSessions),
        Action::Resume(id) => send(Request::Resume(id)),
        Action::SetModel(model) => send(Request::SetModel(model)),
        Action::NewSession => send(Request::NewSession),
        Action::McpSignIn(name) => {
            tokio::spawn(sign_in_to_mcp_server(
                name,
                events.clone(),
                requests.clone(),
            ));
        }
        Action::McpSignOut(name) => {
            tokio::spawn(sign_out_of_mcp_server(
                name,
                events.clone(),
                requests.clone(),
            ));
        }
        Action::Cancel => {
            let _ = cancel.send(());
        }
    }
}
