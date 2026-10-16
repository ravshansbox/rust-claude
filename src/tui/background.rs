use super::{
    App, UiEvent,
    files::{list_files, list_folder},
    prompt_history::send_history,
    workspace::workspace_label,
};
use crate::{history, mcp, settings::Settings, update::Updater};
use std::path::PathBuf;
use tokio::sync::mpsc;

pub(super) fn start_mcp_servers(
    servers: Vec<mcp::StartingServer>,
) -> mpsc::UnboundedReceiver<mcp::Started> {
    let (mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    for server in servers {
        let mcp_tx = mcp_tx.clone();
        tokio::spawn(async move {
            let _ = mcp_tx.send(server.started.await);
        });
    }
    mcp_rx
}

pub(super) fn start_updater(settings: &Settings, events: &mpsc::UnboundedSender<UiEvent>) {
    if let Some(updater) = Updater::for_this_build(settings) {
        let events = events.clone();
        tokio::spawn(async move {
            updater
                .run(move |progress| {
                    let _ = events.send(UiEvent::Update(progress));
                })
                .await;
        });
    }
}

pub(super) fn start_background_work(
    app: &mut App,
    events: &mpsc::UnboundedSender<UiEvent>,
    history: &std::sync::mpsc::Sender<(PathBuf, history::Entry)>,
) {
    if let Some(generation) = app.start_listing_files() {
        send_blocking(events, move || UiEvent::Files(generation, list_files()));
    }
    if let Some((generation, folder)) = app.start_listing_folder() {
        send_blocking(events, move || {
            let names = list_folder(&folder);
            UiEvent::Folder(generation, folder, names)
        });
    }
    if std::mem::take(&mut app.workspace_stale) {
        send_blocking(events, || UiEvent::Workspace(workspace_label()));
    }
    send_history(app, history);
}

fn send_blocking(
    events: &mpsc::UnboundedSender<UiEvent>,
    event: impl FnOnce() -> UiEvent + Send + 'static,
) {
    let events = events.clone();
    tokio::task::spawn_blocking(move || {
        let _ = events.send(event());
    });
}
