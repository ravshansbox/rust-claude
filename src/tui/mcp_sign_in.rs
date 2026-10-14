use super::{Request, UiEvent, worker::send_notice};
use crate::mcp;
use std::process::Stdio;
use tokio::sync::mpsc;

/// Opens `url` in the default browser, if there is one.
fn open_in_browser(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = tokio::process::Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// Signs in to the MCP server `name` in the browser, then starts it again.
pub(super) async fn sign_in_to_mcp_server(
    name: String,
    events: mpsc::UnboundedSender<UiEvent>,
    requests: mpsc::UnboundedSender<Request>,
) {
    let notice = |text| send_notice(&events, text);
    let failed = |error| notice(format!("MCP sign-in to {name} failed: {error}"));
    let sign_in = match mcp::begin_sign_in(&name).await {
        Ok(sign_in) => sign_in,
        Err(error) => return failed(error),
    };
    notice(format!(
        "opening the sign-in page for MCP server {name}; if it does not open, visit {}",
        sign_in.authorize_url
    ));
    open_in_browser(&sign_in.authorize_url);
    if let Err(error) = sign_in.finish().await {
        return failed(error);
    }
    let signed_in = format!("signed in to MCP server {name}");
    restart_mcp_server(name, signed_in, events, requests).await;
}

/// Forgets the sign-in for the MCP server `name`, then starts it again.
pub(super) async fn sign_out_of_mcp_server(
    name: String,
    events: mpsc::UnboundedSender<UiEvent>,
    requests: mpsc::UnboundedSender<Request>,
) {
    match mcp::sign_out(&name) {
        Ok(signed_out) => restart_mcp_server(name, signed_out, events, requests).await,
        Err(error) => send_notice(&events, format!("MCP sign-out of {name} failed: {error}")),
    }
}

/// Shows `notice` and starts the MCP server `name` again in place of the
/// running one.
async fn restart_mcp_server(
    name: String,
    notice: String,
    events: mpsc::UnboundedSender<UiEvent>,
    requests: mpsc::UnboundedSender<Request>,
) {
    let Some(server) = mcp::restart(&name) else {
        return send_notice(&events, notice);
    };
    let _ = events.send(UiEvent::McpRestarting {
        name,
        scope: server.scope,
        notice,
    });
    let _ = requests.send(Request::ReplaceMcpServer(server.started.await));
}

#[cfg(test)]
mod tests;
