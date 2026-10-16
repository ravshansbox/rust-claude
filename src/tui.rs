mod app;
mod background;
mod commands;
mod draw;
mod event_loop;
mod files;
mod input;
mod keys;
mod mcp_sign_in;
mod mcp_status;
mod message;
mod opening;
mod perform;
mod prompt_history;
mod render;
mod replay;
mod selection;
mod settings_sync;
mod status;
mod suggestions;
mod tasks;
mod terminal;
#[cfg(test)]
mod test_support;
mod worker;
mod workspace;

use crate::{agent::Agent, highlight::Colour, restart::Restart, stop::StopSignals};
use anyhow::Result;
use app::{Activity, App, Picker, PickerKind};
use event_loop::run_loop;
use message::Role;
use prompt_history::HistorySearch;
use render::{BACKGROUND, THEME, Theme};
use std::{io, time::SystemTime};
use terminal::{HeldPanics, InputModes};
use worker::{Request, UiEvent, send_notice};
use workspace::workspace_label;

pub(crate) use render::{ansi_thinking, ansi_tool_lines, tool_failure, tool_message, tool_note};
pub(crate) use tasks::finished_task;

/// Whether the terminal has a dark theme, and its background if it says.
pub fn terminal_theme() -> (bool, Option<Colour>) {
    let (theme, background) = Theme::detect();
    (matches!(theme, Theme::Dark), background)
}

/// Runs the interface, which returns how to restart after an update.
pub async fn run(
    agent: Agent,
    stop: StopSignals,
    restarted_input: Option<String>,
) -> Result<Option<Restart>> {
    let (theme, background) = Theme::detect();
    THEME.get_or_init(|| theme);
    BACKGROUND.get_or_init(|| background);
    let mut terminal = ratatui::init();
    let held = HeldPanics::install(io::stderr());
    let modes = match InputModes::enable(io::stdout()) {
        Ok(modes) => modes,
        Err(error) => {
            ratatui::restore();
            drop(held);
            return Err(error.into());
        }
    };
    let result = run_loop(&mut terminal, agent, stop, restarted_input).await;
    drop(modes);
    ratatui::restore();
    drop(held);
    result
}

fn time_ago(time: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(time)
        .unwrap_or_default()
        .as_secs();
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3_600 => format!("{}m ago", seconds / 60),
        3_600..86_400 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn display_model(model: &str) -> &'static str {
    crate::models::name(model)
}

#[cfg(test)]
mod tests;
