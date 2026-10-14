use super::{
    display_model,
    files::FileList,
    keys::Action,
    mcp_status::McpGroup,
    message::{ChatMessage, Role},
    render::tool_message,
    selection::Selection,
    workspace_label,
};
use crate::{
    agent::{Queue, Queued, Stats, take_queued},
    history,
    images::Image,
    models,
    skills::Skill,
    tools,
    update::Progress,
};
use anyhow::Result;
use ratatui::layout::Rect;
use std::{fmt, path::PathBuf};

const VERSION_LINE: &str = concat!("Rust Coding Agent v", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Activity {
    Working,
    Compacting,
    Running,
    Cancelling,
    Resuming,
    StartingNewSession,
    LoadingSessions,
}

impl Activity {
    /// Whether Esc can stop it.
    pub(super) fn can_cancel(self) -> bool {
        matches!(self, Self::Working | Self::Compacting | Self::Running)
    }

    /// Whether prompts sent meanwhile are queued for the model.
    fn can_queue(self) -> bool {
        matches!(self, Self::Working | Self::Compacting)
    }
}

impl fmt::Display for Activity {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(match self {
            Self::Working => "working",
            Self::Compacting => "compacting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Resuming => "resuming",
            Self::StartingNewSession => "starting new session",
            Self::LoadingSessions => "loading sessions",
        })
    }
}

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
    /// What the app was already waiting for when a background turn started
    /// ahead of it, shown again once that turn ends.
    waiting_behind_background: Option<Activity>,
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
    /// Counts prompts sent, so a list read before the last one is dropped.
    pub(super) files_generation: u64,
    pub(super) prompt_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) reads: tools::ReadGroup,
    pub(super) tasks: super::tasks::Tasks,
    pub(super) skills: Vec<Skill>,
    pub(super) mcp_sign_in_servers: Vec<String>,
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
    pub(super) conversation_top: usize,
    version_line: String,
    version_message: Option<usize>,
    pub(super) notifications: bool,
    pub(super) focused: bool,
    /// The message to show in a terminal notification, until it is sent.
    pub(super) notification: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum PickerKind {
    Session,
    Model,
    Effort,
}

pub(super) struct Picker {
    pub(super) kind: PickerKind,
    pub(super) title: &'static str,
    pub(super) items: Vec<(String, String)>,
    pub(super) selected: usize,
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
        if progress.finishes_step()
            && let Some(step) = self.version_line.rfind(" · ")
        {
            self.version_line.truncate(step);
        }
        self.version_line.push_str(" · ");
        self.version_line.push_str(&progress.message());
        self.show_version();
    }

    fn show_version(&mut self) {
        let text = self.version_line.clone();
        self.version_message = Some(self.show_event(self.version_message, text));
    }

    pub(super) fn show_event(&mut self, message: Option<usize>, text: String) -> usize {
        if let Some(message) = message {
            self.messages[message].replace(text);
            message
        } else {
            self.push(Role::Event, text);
            self.messages.len() - 1
        }
    }

    /// Rows left blank above the message: none above the first one or
    /// between notices in a row, one elsewhere.
    pub(super) fn gap_before(&self, index: usize) -> usize {
        let notices = index
            .checked_sub(1)
            .and_then(|previous| self.messages.get(previous))
            .zip(self.messages.get(index))
            .is_some_and(|(previous, message)| {
                matches!((previous.role, message.role), (Role::Event, Role::Event))
            });
        usize::from(index > 0 && !notices)
    }

    pub(super) fn push(&mut self, role: Role, text: impl Into<String>) {
        self.reads.clear();
        if let Some(last) = self.messages.last_mut() {
            last.finish_streaming();
        }
        self.messages.push(ChatMessage {
            role,
            text: text.into(),
            rendered: None,
            streaming: false,
        });
    }

    pub(super) fn set_message_text(&mut self, index: usize, text: String) {
        if let Some(message) = self.messages.get_mut(index) {
            message.replace(text);
        }
    }

    pub(super) fn push_tool(&mut self, name: &str, summary: String, diff: Option<&str>) {
        if name != "read" || diff.is_some() {
            self.push(Role::Tool, tool_message(name, &summary, diff));
            return;
        }
        let merge = !self.reads.is_empty();
        let mut reads = std::mem::take(&mut self.reads);
        reads.add(summary);
        let text = tool_message(name, &reads.summary(), None);
        match self.messages.last_mut() {
            Some(last) if merge => last.replace(text),
            _ => self.push(Role::Tool, text),
        }
        self.reads = reads;
    }

    pub(super) fn push_shell(&mut self, command: &str, output: &str) {
        let output = output.trim_end();
        let diff = (!output.is_empty()).then_some(output);
        self.push(Role::Tool, tool_message("!", command, diff));
    }

    pub(super) fn push_background(&mut self, message: &str) -> bool {
        let Some((summary, output)) = tools::background::display(message) else {
            return false;
        };
        self.push(
            Role::Tool,
            tool_message("background", &summary, output.as_deref()),
        );
        true
    }

    /// Shows the picker in place of the prompt history search, so keys go
    /// to the list on screen. The input stays as it was.
    pub(super) fn open_picker(&mut self, picker: Picker) {
        self.history_search = None;
        self.picker = Some(picker);
    }

    pub(super) fn scroll_up(&mut self, amount: usize) {
        self.scroll_from_bottom = self
            .scroll_from_bottom
            .saturating_add(amount)
            .min(self.max_scroll);
    }

    pub(super) fn scroll_down(&mut self, amount: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(amount);
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.scroll_from_bottom = self.max_scroll;
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }

    pub(super) fn cycle_effort(&mut self) {
        let levels = models::effort_levels(&self.model);
        let index = levels
            .iter()
            .position(|level| *level == self.effort)
            .map_or(0, |index| (index + 1) % levels.len());
        self.effort = levels[index];
    }

    pub(super) fn input_changed(&mut self) {
        self.history_index = None;
        self.command_selected = 0;
        self.commands_dismissed = false;
    }

    pub(super) fn attach_image(&mut self, image: Image) {
        self.image_count += 1;
        self.insert(&image_marker(self.image_count));
        self.images.push((self.image_count, image));
    }

    pub(super) fn take_numbered_images(&mut self, prompt: &str) -> Vec<(usize, Image)> {
        std::mem::take(&mut self.images)
            .into_iter()
            .filter(|(number, _)| prompt.contains(&image_marker(*number)))
            .collect()
    }

    pub(super) fn busy(&self) -> bool {
        self.activity.is_some()
    }

    pub(super) fn animating(&self) -> bool {
        self.busy() || self.mcp_loading()
    }

    pub(super) fn tick_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        self.show_loading_mcp_groups();
    }
    pub(super) fn can_queue(&self) -> bool {
        self.activity.is_some_and(Activity::can_queue)
    }

    pub(super) fn queue_prompt(&mut self) {
        let prompt = self.take_input();
        self.remember(&prompt);
        let images = self.take_numbered_images(&prompt);
        if let Ok(mut queue) = self.queue.lock() {
            queue.push(Queued { prompt, images });
        }
    }

    pub(super) fn send_queued(&mut self) -> Option<(String, Vec<Image>)> {
        let queued = take_queued(&self.queue);
        if queued.is_empty() {
            return None;
        }
        let prompt = queued
            .iter()
            .map(|queued| queued.prompt.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let images = queued
            .into_iter()
            .flat_map(|queued| queued.images)
            .map(|(_, image)| image)
            .collect();
        self.push(Role::User, prompt.clone());
        self.start(Activity::Working);
        Some((prompt, images))
    }

    pub(super) fn restore_queued(&mut self) {
        let queued = take_queued(&self.queue);
        if queued.is_empty() {
            return;
        }
        let mut parts = Vec::new();
        for queued in queued {
            parts.push(queued.prompt);
            self.images.extend(queued.images);
        }
        if !self.input.is_empty() {
            parts.push(std::mem::take(&mut self.input));
        }
        self.input = parts.join("\n\n");
        self.cursor = self.input.len();
        self.input_changed();
    }

    /// Puts back text the user entered so they can fix it.
    pub(super) fn take_input(&mut self) -> String {
        self.cursor = 0;
        self.forget_files();
        self.history_index = None;
        std::mem::take(&mut self.input)
    }

    pub(super) fn insert(&mut self, text: &str) {
        self.input.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.input_changed();
    }

    pub(super) fn delete_to(&mut self, start: usize) {
        self.input.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.input_changed();
    }

    pub(super) fn submit(&mut self, prompt: String) -> Action {
        self.remember(&prompt);
        self.start(Activity::Working);
        let images = self.take_numbered_images(&prompt);
        let images = images.into_iter().map(|(_, image)| image).collect();
        Action::Submit(prompt, images, self.effort)
    }

    pub(super) fn restore_input(&mut self, text: String) {
        self.input = text;
        self.cursor = self.input.len();
        self.input_changed();
    }

    pub(super) fn start(&mut self, activity: Activity) {
        self.activity = Some(activity);
    }

    /// Starts a turn for a finished background command. The worker may start
    /// it just as a request is sent, which then waits behind it.
    pub(super) fn start_background(&mut self) {
        self.waiting_behind_background = self.activity;
        self.start(Activity::Working);
    }

    /// Returns whether `name` is an effort level.
    pub(super) fn set_effort(&mut self, name: &str) -> bool {
        if let Some(level) = models::find_effort(&self.model, name) {
            self.effort = level;
            self.push(Role::Event, format!("effort: {level}"));
            true
        } else {
            self.push(
                Role::Event,
                format!(
                    "unknown effort level: {name} (options: {})",
                    models::effort_levels(&self.model).join(", ")
                ),
            );
            false
        }
    }

    pub(super) fn set_model(&mut self, model: &str) {
        self.model = model.into();
        self.effort = models::supported_effort(model, self.effort);
        self.push(Role::Event, format!("model: {}", display_model(model)));
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

    pub(super) fn finish<T>(&mut self, result: Result<T>, on_success: impl FnOnce(&mut Self, T)) {
        if let Some(last) = self.messages.last_mut() {
            last.finish_streaming();
        }
        match result {
            Ok(value) => on_success(self, value),
            Err(error) => self.push(Role::Event, format!("error: {error:#}")),
        }
        self.activity = self.waiting_behind_background.take();
    }
}

pub(super) fn image_marker(number: usize) -> String {
    format!("[image {number}]")
}

#[cfg(test)]
mod tests;
