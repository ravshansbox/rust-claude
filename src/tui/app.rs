use super::{
    Role,
    files::{FileList, FolderList},
    mcp_status::McpGroup,
    message::ChatMessage,
    selection::Selection,
    workspace_label,
};
use crate::{
    agent::{Queue, Stats},
    history,
    images::Image,
    mcp,
    skills::Skill,
    tools,
    update::Progress,
};
use ratatui::layout::Rect;
use std::path::PathBuf;

mod activity;
mod input;
mod looping;
mod messages;
mod model;
mod picker;
mod queue;
mod scroll;
mod tool_messages;

pub(super) use activity::Activity;
pub(super) use picker::{Picker, PickerKind};

const VERSION_LINE: &str = concat!("Rust Coding Agent v", env!("CARGO_PKG_VERSION"));

#[derive(Default)]
pub(super) struct App {
    pub(super) input: String,
    pub(super) cursor: usize,
    pub(super) messages: Vec<ChatMessage>,
    pub(super) workspace: String,
    /// Set after a command that may have switched branches, until the run
    /// loop starts reading the folder and branch again.
    pub(super) workspace_stale: bool,
    pub(super) model: String,
    pub(super) effort: &'static str,
    /// What the app is waiting for, shown next to the spinner. `None` when idle.
    pub(super) activity: Option<Activity>,
    pub(super) spinner_frame: usize,
    pub(super) stats: Stats,
    pub(super) scroll_from_bottom: usize,
    pub(super) max_scroll: usize,
    pub(super) status_pinned: bool,
    pub(super) page_size: usize,
    pub(super) input_width: usize,
    pub(super) picker: Option<Picker>,
    pub(super) command_selected: usize,
    pub(super) commands_dismissed: bool,
    pub(super) files: Option<FileList>,
    pub(super) listing_files: bool,
    pub(super) folder: Option<FolderList>,
    pub(super) listing_folder: Option<String>,
    /// Counts prompts sent, so a list read before the last one is dropped.
    pub(super) files_generation: u64,
    pub(super) prompt_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) reads: tools::ReadGroup,
    pub(super) tasks: super::tasks::Tasks,
    /// The running task whose agent is shown instead of the main session.
    pub(super) agent_view: Option<usize>,
    pub(super) skills: Vec<Skill>,
    pub(super) mcp_sign_in_servers: Vec<mcp::SignInServer>,
    pub(super) images: Vec<(usize, Image)>,
    pub(super) image_count: usize,
    pub(super) queue: Queue,
    /// Every prompt in the history file and sent since, oldest first.
    pub(super) history: Vec<history::Entry>,
    pub(super) history_file: Option<PathBuf>,
    /// Prompts to add to the history file, which happens off the interface.
    pub(super) unsaved_history: Vec<(PathBuf, history::Entry)>,
    pub(super) history_search: Option<super::prompt_history::HistorySearch>,
    pub(super) mcp_groups: Vec<McpGroup>,
    pub(super) selection: Option<Selection>,
    /// Where the conversation was last drawn, and its first row shown.
    pub(super) conversation_area: Rect,
    pub(super) bottom_button: Rect,
    pub(super) suggestions_area: Rect,
    pub(super) conversation_top: usize,
    version_line: String,
    version_message: Option<usize>,
    pub(super) notifications: bool,
    pub(super) focused: bool,
    /// The message to show in a terminal notification, until it is sent.
    pub(super) notification: Option<String>,
    /// The installed update to restart with once idle.
    restart_program: Option<PathBuf>,
    looping: Option<looping::Loop>,
}

impl App {
    pub(super) fn new(model: &str, effort: &'static str, stats: Stats) -> Self {
        let mut app = Self {
            workspace: workspace_label(),
            model: model.into(),
            effort,
            stats,
            page_size: 1,
            input_width: usize::MAX,
            version_line: VERSION_LINE.into(),
            notifications: true,
            focused: true,
            ..Self::default()
        };
        app.show_version();
        app
    }

    pub(super) fn show_update(&mut self, progress: &Progress) {
        if let Progress::Ready(Some(program)) = progress {
            self.restart_program = Some(program.clone());
        }
        if progress.finishes_step()
            && let Some(step) = self.version_line.rfind(" · ")
        {
            self.version_line.truncate(step);
        }
        self.version_line.push_str(" · ");
        self.version_line.push_str(&progress.message());
        self.show_version();
    }

    /// The installed update to restart with, once no reply or queued prompt
    /// is waiting and loop mode is off.
    pub(super) fn restart_due(&self) -> Option<PathBuf> {
        let queued = self.queue.lock().map_or(0, |queue| queue.len());
        (!self.busy() && queued == 0 && self.looping.is_none())
            .then(|| self.restart_program.clone())
            .flatten()
    }

    pub(super) fn restored_after_update(&mut self, input: String) {
        self.push(
            Role::Event,
            concat!("restarted after update to v", env!("CARGO_PKG_VERSION")),
        );
        self.cursor = input.len();
        self.input = input;
    }

    fn show_version(&mut self) {
        let text = self.version_line.clone();
        self.version_message = Some(self.show_event(self.version_message, text));
    }

    pub(super) fn clear_session(&mut self) {
        self.messages.clear();
        self.version_message = None;
        self.selection = None;
        for group in &mut self.mcp_groups {
            group.message = None;
        }
        self.history_index = None;
    }
}

#[cfg(test)]
mod tests;
