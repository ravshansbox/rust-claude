use super::{
    commands::command_matches,
    display_model,
    draw::SPINNER_FRAMES,
    files::{FileList, file_query},
    render::{render_message, tool_message, wrapped_height},
    selection::Selection,
    workspace_label,
};
use crate::{
    agent::{EFFORT_LEVELS, Queue, Queued, Stats, take_queued},
    history,
    images::Image,
    skills::{Scope, Skill},
    tools,
};
use anyhow::Result;
use ratatui::{layout::Rect, text::Line};
use std::{path::PathBuf, time::Instant};

pub(super) struct Suggestions {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) items: Vec<(String, String)>,
    pub(super) files: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Role {
    User,
    Assistant,
    Thinking,
    Tool,
    Event,
}

pub(super) struct ChatMessage {
    pub(super) role: Role,
    pub(super) text: String,
    pub(super) rendered: Option<Rendered>,
}

pub(super) struct Rendered {
    pub(super) width: u16,
    pub(super) lines: Vec<Line<'static>>,
    /// Rows each line takes once wrapped to `width`.
    pub(super) heights: Vec<usize>,
    pub(super) height: usize,
    /// Bytes of text these lines show.
    text_len: usize,
    /// Bytes of text, and lines showing them, that stay the same as a reply
    /// streams in: everything up to its last closed code block.
    finished_text: usize,
    finished_lines: usize,
}

impl Rendered {
    fn new(width: u16) -> Self {
        Self {
            width,
            lines: Vec::new(),
            heights: Vec::new(),
            height: 0,
            text_len: 0,
            finished_text: 0,
            finished_lines: 0,
        }
    }

    /// Adds the lines of the markdown that follows. Separately rendered
    /// blocks need the blank line that markdown puts between blocks.
    fn extend(&mut self, lines: Vec<Line<'static>>) {
        if !lines.is_empty() && !self.lines.is_empty() {
            self.lines.push(Line::default());
            self.heights.push(1);
        }
        for line in lines {
            let height = wrapped_height(&line, self.width);
            self.lines.push(line);
            self.heights.push(height);
        }
        self.height = self.heights.iter().sum();
    }

    /// Wraps the same lines to a new width.
    fn rewrap(&mut self, width: u16) {
        self.width = width;
        self.heights = self
            .lines
            .iter()
            .map(|line| wrapped_height(line, width))
            .collect();
        self.height = self.heights.iter().sum();
    }
}

fn group_text(spinner_frame: usize, group: &McpGroup) -> String {
    let frame = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
    let entries = group
        .servers
        .iter()
        .map(|server| match &server.summary {
            Some(summary) => summary.clone(),
            None => format!("{frame} {}", server.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let scope = group.scope;
    if group.loading() {
        format!("{scope} MCP servers: {entries}")
    } else {
        format!("loaded {scope} MCP servers: {entries}")
    }
}

impl ChatMessage {
    pub(super) fn append(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn replace(&mut self, text: String) {
        self.text = text;
        self.rendered = None;
    }

    /// Renders the text if it changed. A streaming reply keeps the lines of
    /// its closed code blocks and what comes before them, since highlighting
    /// code is slow, and renders only what follows. A new width keeps the
    /// lines of messages that look the same at every width.
    pub(super) fn render(&mut self, width: u16) {
        if let Some(rendered) = &mut self.rendered
            && rendered.width != width
            && !depends_on_width(self.role, &self.text)
        {
            rendered.rewrap(width);
        }
        if self
            .rendered
            .as_ref()
            .is_some_and(|rendered| rendered.width == width && rendered.text_len == self.text.len())
        {
            return;
        }
        let mut rendered = match self.rendered.take() {
            Some(rendered) if rendered.width == width => rendered,
            _ => Rendered::new(width),
        };
        rendered.lines.truncate(rendered.finished_lines);
        rendered.heights.truncate(rendered.finished_lines);
        if matches!(self.role, Role::Assistant) {
            let start = rendered.finished_text;
            let finished = start + finished_markdown_len(&self.text[start..]);
            if finished > start {
                rendered.extend(render_message(
                    self.role,
                    &self.text[start..finished],
                    width,
                ));
                rendered.finished_text = finished;
                rendered.finished_lines = rendered.lines.len();
            }
        }
        let rest = &self.text[rendered.finished_text..];
        rendered.extend(render_message(self.role, rest, width));
        rendered.text_len = self.text.len();
        self.rendered = Some(rendered);
    }

    /// Rows the message takes once rendered and wrapped.
    pub(super) fn height(&self) -> usize {
        self.rendered.as_ref().map_or(0, |rendered| rendered.height)
    }
}

/// Whether the message's lines change with the width: user messages are
/// padded to it and replies fit their tables to it.
fn depends_on_width(role: Role, text: &str) -> bool {
    match role {
        Role::User => true,
        Role::Assistant => has_table(text),
        Role::Thinking | Role::Tool | Role::Event => false,
    }
}

/// Whether the markdown may hold a table, by looking for a line that could
/// be its delimiter row, such as `| --- | :-: |`, also inside a quote.
fn has_table(text: &str) -> bool {
    text.lines().any(|line| {
        let row = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
        row.contains('|')
            && row.contains('-')
            && row
                .chars()
                .all(|c| matches!(c, '|' | '-' | ':') || c.is_whitespace())
    })
}

/// Bytes of markdown up to the end of its last closed code block whose
/// opening fence starts the line. Nothing after it changes how the text
/// before it renders.
fn finished_markdown_len(text: &str) -> usize {
    let mut finished = 0;
    // Whether `hard_line_breaks` sees a fence open, by its simpler rule.
    let mut in_fence = false;
    // The fence character and length of the open code block that starts the line.
    let mut open: Option<(char, usize)> = None;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        end += line.len();
        let trimmed = line.trim_start();
        let Some(fence) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
            continue;
        };
        let length = trimmed.chars().take_while(|c| *c == fence).count();
        if length < 3 {
            continue;
        }
        in_fence = !in_fence;
        let indent = line.len() - trimmed.len();
        match open {
            None if indent == 0 => open = Some((fence, length)),
            Some((open_fence, open_length))
                if indent <= 3
                    && fence == open_fence
                    && length >= open_length
                    && trimmed[length..].trim().is_empty() =>
            {
                open = None;
                if !in_fence && line.ends_with('\n') {
                    finished = end;
                }
            }
            _ => {}
        }
    }
    finished
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Activity {
    Working,
    Compacting,
    Running,
    LoadingModels,
    CheckingModel,
    Cancelling,
    Resuming,
    StartingNewSession,
    MeasuringContext,
    LoadingSessions,
}

impl Activity {
    /// Whether Esc can stop it.
    pub(super) fn can_cancel(self) -> bool {
        matches!(
            self,
            Self::Working
                | Self::Compacting
                | Self::Running
                | Self::LoadingModels
                | Self::CheckingModel
        )
    }

    /// Whether prompts sent meanwhile are queued for the model.
    fn can_queue(self) -> bool {
        matches!(self, Self::Working | Self::Compacting)
    }
}

impl std::fmt::Display for Activity {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Self::Working => "working",
            Self::Compacting => "compacting",
            Self::Running => "running",
            Self::LoadingModels => "loading models",
            Self::CheckingModel => "checking model",
            Self::Cancelling => "cancelling",
            Self::Resuming => "resuming",
            Self::StartingNewSession => "starting new session",
            Self::MeasuringContext => "measuring context",
            Self::LoadingSessions => "loading sessions",
        })
    }
}

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
    pub(super) page_size: usize,
    pub(super) input_width: usize,
    pub(super) picker: Option<Picker>,
    pub(super) command_selected: usize,
    pub(super) commands_dismissed: bool,
    pub(super) files: Option<FileList>,
    pub(super) listing_files: bool,
    /// Counts prompts sent, so a list read before the last one is dropped.
    files_generation: u64,
    pub(super) prompt_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) reads: tools::ReadGroup,
    pub(super) skills: Vec<Skill>,
    pub(super) mcp_sign_in_servers: Vec<String>,
    pub(super) images: Vec<(usize, Image)>,
    pub(super) image_count: usize,
    pub(super) queue: Queue,
    /// Every prompt in the history file and sent since, oldest first.
    pub(super) history: Vec<history::Entry>,
    pub(super) history_file: Option<PathBuf>,
    pub(super) history_search: Option<HistorySearch>,
    mcp_groups: Vec<McpGroup>,
    pub(super) selection: Option<Selection>,
    /// Where the conversation was last drawn, and its first row shown.
    pub(super) conversation_area: Rect,
    pub(super) conversation_top: usize,
    /// When the notice that text was copied goes away.
    pub(super) copied_until: Option<Instant>,
}

struct McpGroup {
    scope: Scope,
    servers: Vec<McpEntry>,
    message: Option<usize>,
}

impl McpGroup {
    fn loading(&self) -> bool {
        self.servers.iter().any(|server| server.summary.is_none())
    }
}

struct McpEntry {
    name: String,
    summary: Option<String>,
}

pub(super) struct HistorySearch {
    pub(super) all: bool,
    pub(super) query: String,
    pub(super) current: Vec<String>,
    pub(super) everywhere: Vec<history::Entry>,
    pub(super) selected: usize,
}

impl HistorySearch {
    pub(super) fn matches(&self) -> Vec<(&str, Option<&str>)> {
        let query = self.query.to_lowercase();
        let entries: Vec<(&str, Option<&str>)> = if self.all {
            self.everywhere
                .iter()
                .map(|entry| (entry.prompt.as_str(), Some(entry.folder.as_str())))
                .collect()
        } else {
            self.current
                .iter()
                .map(|prompt| (prompt.as_str(), None))
                .collect()
        };
        entries
            .into_iter()
            .filter(|(prompt, _)| prompt.to_lowercase().contains(&query))
            .collect()
    }
}

#[derive(Clone, Copy)]
pub(super) enum PickerKind {
    Session,
    Model,
    Thinking,
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
            input: String::new(),
            cursor: 0,
            messages: Vec::new(),
            workspace: workspace_label(),
            workspace_stale: false,
            model: model.into(),
            effort,
            activity: None,
            spinner_frame: 0,
            stats,
            scroll_from_bottom: 0,
            max_scroll: 0,
            page_size: 1,
            input_width: usize::MAX,
            picker: None,
            command_selected: 0,
            commands_dismissed: false,
            files: None,
            listing_files: false,
            files_generation: 0,
            prompt_history: Vec::new(),
            history_index: None,
            reads: tools::ReadGroup::default(),
            skills: Vec::new(),
            mcp_sign_in_servers: Vec::new(),
            images: Vec::new(),
            image_count: 0,
            queue: Queue::default(),
            history: Vec::new(),
            history_file: None,
            history_search: None,
            mcp_groups: Vec::new(),
            selection: None,
            conversation_area: Rect::default(),
            conversation_top: 0,
            copied_until: None,
        };
        app.push(
            Role::Event,
            concat!("rust-claude v", env!("CARGO_PKG_VERSION")),
        );
        app
    }

    pub(super) fn push(&mut self, role: Role, text: impl Into<String>) {
        self.reads.clear();
        self.messages.push(ChatMessage {
            role,
            text: text.into(),
            rendered: None,
        });
    }

    pub(super) fn push_tool(&mut self, name: &str, summary: String, diff: Option<String>) {
        if name != "read" || diff.is_some() {
            self.push(Role::Tool, tool_message(name, summary, diff));
            return;
        }
        let merge = !self.reads.is_empty();
        let mut reads = std::mem::take(&mut self.reads);
        reads.add(summary);
        let text = tool_message(name, reads.summary(), None);
        match self.messages.last_mut() {
            Some(last) if merge => {
                last.text = text;
                last.rendered = None;
            }
            _ => self.push(Role::Tool, text),
        }
        self.reads = reads;
    }

    pub(super) fn push_shell(&mut self, command: &str, output: &str) {
        let output = output.trim_end();
        let diff = (!output.is_empty()).then(|| output.to_string());
        self.push(Role::Tool, tool_message("!", command.to_string(), diff));
    }

    /// Loads the prompt history once; prompts sent later are added to it.
    pub(super) fn load_history(&mut self, path: Option<PathBuf>) {
        self.history = path.as_deref().map(history::load).unwrap_or_default();
        if let Some(path) = &path
            && let Err(error) = history::trim(path, &mut self.history)
        {
            self.push(
                Role::Event,
                format!("failed to trim prompt history: {error:#}"),
            );
        }
        self.prompt_history =
            history::folder_prompts(&self.history, &crate::session::current_folder());
        self.history_file = path;
    }

    pub(super) fn add_prompt(&mut self, prompt: String) {
        self.prompt_history.retain(|existing| *existing != prompt);
        self.prompt_history.push(prompt);
    }

    pub(super) fn remember(&mut self, prompt: &str) {
        self.add_prompt(prompt.to_string());
        let entry = history::Entry::here(prompt);
        let saved = self
            .history_file
            .as_deref()
            .map(|path| history::append(path, &entry));
        self.history.push(entry);
        if let Some(Err(error)) = saved {
            self.push(
                Role::Event,
                format!("failed to save prompt history: {error:#}"),
            );
        }
    }

    pub(super) fn open_history_search(&mut self) {
        let current = self.prompt_history.iter().rev().cloned().collect();
        self.history_search = Some(HistorySearch {
            all: false,
            query: String::new(),
            current,
            everywhere: history::newest_unique(&self.history),
            selected: 0,
        });
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

    /// Recalls the prompt before. Suggestions for a recalled command or
    /// `@` stay hidden until the input is edited, so Up and Down keep
    /// browsing.
    pub(super) fn previous_prompt(&mut self) {
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None if self.prompt_history.is_empty() => return,
            None => self.prompt_history.len() - 1,
        };
        self.history_index = Some(index);
        self.input = self.prompt_history[index].clone();
        self.cursor = self.input.len();
        self.commands_dismissed = true;
    }

    pub(super) fn next_prompt(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.prompt_history.len() {
            self.history_index = Some(index + 1);
            self.input = self.prompt_history[index + 1].clone();
        } else {
            self.history_index = None;
            self.input.clear();
        }
        self.cursor = self.input.len();
        self.commands_dismissed = self.history_index.is_some();
    }

    pub(super) fn visible_suggestions(&self) -> Option<Suggestions> {
        if self.commands_dismissed {
            return None;
        }
        let commands = command_matches(&self.input, &self.skills, &self.mcp_sign_in_servers);
        if !commands.is_empty() {
            return Some(Suggestions {
                start: 0,
                end: self.input.len(),
                items: commands,
                files: false,
            });
        }
        let (start, query) = file_query(&self.input, self.cursor)?;
        let items: Vec<(String, String)> = self
            .files
            .as_ref()?
            .matches(query)
            .into_iter()
            .map(|path| (path, String::new()))
            .collect();
        if items.is_empty() {
            return None;
        }
        // Accepting replaces the whole `@` word, including any part after
        // the cursor, so no tail of the old name is left behind.
        let end = self.input[self.cursor..]
            .find(char::is_whitespace)
            .map_or(self.input.len(), |offset| self.cursor + offset);
        Some(Suggestions {
            start,
            end,
            items,
            files: true,
        })
    }

    pub(super) fn accept_suggestion(&mut self, suggestions: &Suggestions) {
        let name = &suggestions.items[self.command_selected].0;
        let replacement = if suggestions.files {
            format!("@{name} ")
        } else {
            name.clone()
        };
        self.input
            .replace_range(suggestions.start..suggestions.end, &replacement);
        self.cursor = suggestions.start + replacement.len();
    }

    pub(super) fn cycle_effort(&mut self) {
        let index = EFFORT_LEVELS
            .iter()
            .position(|level| *level == self.effort)
            .map_or(0, |index| (index + 1) % EFFORT_LEVELS.len());
        self.effort = EFFORT_LEVELS[index];
    }

    pub(super) fn input_changed(&mut self) {
        self.history_index = None;
        self.command_selected = 0;
        self.commands_dismissed = false;
    }

    /// Whether the file list should start loading now, for an `@` the user
    /// just typed. Returns the list's generation once until the list arrives.
    pub(super) fn start_listing_files(&mut self) -> Option<u64> {
        if self.files.is_some()
            || self.listing_files
            || file_query(&self.input, self.cursor).is_none()
        {
            return None;
        }
        self.listing_files = true;
        Some(self.files_generation)
    }

    pub(super) fn set_files(&mut self, generation: u64, files: Vec<String>) {
        if generation == self.files_generation {
            self.files = Some(FileList::new(files));
            self.listing_files = false;
        }
    }

    /// Makes the next `@` read the files again, as the prompt may change them.
    pub(super) fn forget_files(&mut self) {
        self.files = None;
        self.listing_files = false;
        self.files_generation += 1;
    }

    pub(super) fn attach_image(&mut self, image: Image) {
        self.image_count += 1;
        let marker = image_marker(self.image_count);
        self.input.insert_str(self.cursor, &marker);
        self.cursor += marker.len();
        self.images.push((self.image_count, image));
        self.input_changed();
    }

    pub(super) fn take_images(&mut self, prompt: &str) -> Vec<Image> {
        self.take_numbered_images(prompt)
            .into_iter()
            .map(|(_, image)| image)
            .collect()
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
        self.busy() || self.mcp_groups.iter().any(McpGroup::loading)
    }

    pub(super) fn tick_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        for index in 0..self.mcp_groups.len() {
            if self.mcp_groups[index].loading() {
                self.show_mcp_group(index);
            }
        }
    }

    /// Shows the servers loading, one line for each scope, global first.
    pub(super) fn start_mcp_servers<'a>(
        &mut self,
        servers: impl IntoIterator<Item = (&'a str, Scope)>,
    ) {
        let mut changed = Vec::new();
        for (name, scope) in servers {
            let found = self
                .mcp_groups
                .iter_mut()
                .enumerate()
                .find_map(|(index, group)| {
                    let server = group
                        .servers
                        .iter_mut()
                        .find(|server| server.name == name)?;
                    Some((index, server))
                });
            if let Some((index, server)) = found {
                server.summary = None;
                changed.push(index);
                continue;
            }
            let index = match self
                .mcp_groups
                .iter()
                .position(|group| group.scope == scope)
            {
                Some(index) => index,
                None => {
                    self.mcp_groups.push(McpGroup {
                        scope,
                        servers: Vec::new(),
                        message: None,
                    });
                    self.mcp_groups
                        .sort_by_key(|group| matches!(group.scope, Scope::Project));
                    self.mcp_groups
                        .iter()
                        .position(|group| group.scope == scope)
                        .unwrap_or_default()
                }
            };
            self.mcp_groups[index].servers.push(McpEntry {
                name: name.into(),
                summary: None,
            });
        }
        for index in 0..self.mcp_groups.len() {
            if self.mcp_groups[index].message.is_none() || changed.contains(&index) {
                self.show_mcp_group(index);
            }
        }
    }

    pub(super) fn finish_mcp_server(&mut self, added: crate::mcp::Added) {
        let found = self
            .mcp_groups
            .iter_mut()
            .enumerate()
            .find_map(|(index, group)| {
                let server = group
                    .servers
                    .iter_mut()
                    .find(|server| server.name == added.name)?;
                Some((index, server))
            });
        match found {
            Some((index, server)) => {
                server.summary = Some(added.summary);
                self.show_mcp_group(index);
                if added.failed {
                    self.push(Role::Event, added.status);
                }
            }
            None => self.push(Role::Event, added.status),
        }
    }

    fn show_mcp_group(&mut self, index: usize) {
        let text = group_text(self.spinner_frame, &self.mcp_groups[index]);
        match self.mcp_groups[index].message {
            Some(message) => self.messages[message].replace(text),
            None => {
                self.push(Role::Event, text);
                self.mcp_groups[index].message = Some(self.messages.len() - 1);
            }
        }
    }

    pub(super) fn can_queue(&self) -> bool {
        self.activity.is_some_and(Activity::can_queue)
    }

    pub(super) fn queue_prompt(&mut self) {
        let prompt = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.forget_files();
        self.history_index = None;
        self.remember(&prompt);
        let images = self.take_numbered_images(&prompt);
        if let Ok(mut queue) = self.queue.lock() {
            queue.push(Queued { prompt, images });
        }
    }

    pub(super) fn queued_prompts(&self) -> Vec<String> {
        self.queue
            .lock()
            .map(|queue| queue.iter().map(|queued| queued.prompt.clone()).collect())
            .unwrap_or_default()
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
        let mut parts: Vec<String> = Vec::new();
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
    pub(super) fn restore_input(&mut self, text: String) {
        self.input = text;
        self.cursor = self.input.len();
        self.input_changed();
    }

    pub(super) fn start(&mut self, activity: Activity) {
        self.activity = Some(activity);
    }

    /// Returns whether `name` is an effort level.
    pub(super) fn set_effort(&mut self, name: &str) -> bool {
        match EFFORT_LEVELS.iter().find(|level| **level == name) {
            Some(level) => {
                self.effort = level;
                self.push(Role::Event, format!("thinking: {level}"));
                true
            }
            None => {
                self.push(
                    Role::Event,
                    format!(
                        "unknown effort level: {name} (options: {})",
                        EFFORT_LEVELS.join(", ")
                    ),
                );
                false
            }
        }
    }

    pub(super) fn set_model(&mut self, model: &str) {
        self.model = model.into();
        self.push(Role::Event, format!("model: {}", display_model(model)));
    }

    pub(super) fn clear_session(&mut self) {
        self.messages.clear();
        self.selection = None;
        for group in &mut self.mcp_groups {
            group.message = None;
        }
        self.history_index = None;
    }

    pub(super) fn finish<T>(&mut self, result: Result<T>, on_success: impl FnOnce(&mut Self, T)) {
        match result {
            Ok(value) => on_success(self, value),
            Err(error) => self.push(Role::Event, format!("error: {error:#}")),
        }
        self.activity = None;
    }
}

pub(super) fn image_marker(number: usize) -> String {
    format!("[image {number}]")
}

#[cfg(test)]
mod tests {
    use super::{Activity, App, ChatMessage, Role, Scope};
    use crate::agent::AgentEvent;
    use crate::tui::{
        UiEvent, handle_agent_event, handle_input,
        test_support::{new_app, screen},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent};
    use ratatui::text::Line;

    fn mcp_server_loaded(name: &str, summary: &str) -> UiEvent {
        UiEvent::McpServer(crate::mcp::Added {
            name: name.into(),
            status: format!("loaded MCP server: {summary}"),
            summary: summary.into(),
            failed: false,
            diagnostics: Vec::new(),
        })
    }

    fn mcp_server_failed(name: &str, summary: &str, status: &str) -> UiEvent {
        UiEvent::McpServer(crate::mcp::Added {
            name: name.into(),
            status: status.into(),
            summary: summary.into(),
            failed: true,
            diagnostics: Vec::new(),
        })
    }

    #[test]
    fn shows_name_and_version_before_start_up_lines() {
        let mut app = new_app();
        app.push(Role::Event, "loaded AGENTS.md");
        let shown = screen(&mut app);
        let name = shown
            .find(&format!("rust-claude v{}", env!("CARGO_PKG_VERSION")))
            .expect(&shown);
        let loaded = shown.find("loaded AGENTS.md").expect(&shown);
        assert!(name < loaded, "{shown}");
    }

    #[test]
    fn groups_mcp_servers_by_scope_on_two_lines_with_spinners() {
        let mut app = new_app();
        app.start_mcp_servers([
            ("docs", Scope::Project),
            ("figma", Scope::Global),
            ("web", Scope::Global),
        ]);
        app.push(Role::User, "hello");
        let shown = screen(&mut app);
        let global = shown
            .find("global MCP servers: ⠋ figma, ⠋ web")
            .expect(&shown);
        let project = shown.find("project MCP servers: ⠋ docs").expect(&shown);
        let hello = shown.find("hello").expect(&shown);
        assert!(global < project && project < hello, "{shown}");
        handle_agent_event(mcp_server_loaded("web", "web (3 tools)"), &mut app);
        let shown = screen(&mut app);
        assert!(
            shown.contains("global MCP servers: ⠋ figma, web (3 tools)"),
            "{shown}"
        );
        handle_agent_event(
            mcp_server_failed(
                "figma",
                "figma (needs sign-in)",
                "MCP server figma needs sign-in: run /mcp login figma",
            ),
            &mut app,
        );
        let shown = screen(&mut app);
        let global = shown
            .find("loaded global MCP servers: figma (needs sign-in), web (3 tools)")
            .expect(&shown);
        let project = shown.find("project MCP servers: ⠋ docs").expect(&shown);
        let hello = shown.find("hello").expect(&shown);
        let failure = shown
            .find("MCP server figma needs sign-in: run /mcp login figma")
            .expect(&shown);
        assert!(
            global < project && project < hello && hello < failure,
            "{shown}"
        );
    }

    #[test]
    fn animates_mcp_servers_loading() {
        let mut app = new_app();
        app.start_mcp_servers([("docs", Scope::Project)]);
        assert!(app.animating());
        app.tick_spinner();
        let shown = screen(&mut app);
        assert!(shown.contains("project MCP servers: ⠙ docs"), "{shown}");
        handle_agent_event(mcp_server_loaded("docs", "docs (3 tools)"), &mut app);
        assert!(!app.animating());
        assert!(screen(&mut app).contains("loaded project MCP servers: docs (3 tools)"));
    }

    #[test]
    fn shows_a_signed_in_server_loading_again_in_its_group() {
        let mut app = new_app();
        app.start_mcp_servers([("figma", Scope::Global)]);
        handle_agent_event(
            mcp_server_failed("figma", "figma (needs sign-in)", "needs sign-in"),
            &mut app,
        );
        handle_agent_event(
            UiEvent::McpRestarting {
                name: "figma".into(),
                scope: Scope::Global,
                notice: "signed in to MCP server figma".into(),
            },
            &mut app,
        );
        let shown = screen(&mut app);
        let group = shown.find("global MCP servers: ⠋ figma").expect(&shown);
        let notice = shown.find("signed in to MCP server figma").expect(&shown);
        assert!(group < notice, "{shown}");
        handle_agent_event(mcp_server_loaded("figma", "figma (5 tools)"), &mut app);
        let shown = screen(&mut app);
        assert!(
            shown.contains("loaded global MCP servers: figma (5 tools)"),
            "{shown}"
        );
        assert_eq!(shown.matches("MCP servers:").count(), 1, "{shown}");
    }

    #[test]
    fn shows_mcp_servers_again_after_clearing_the_session() {
        let mut app = new_app();
        app.start_mcp_servers([("docs", Scope::Project)]);
        app.clear_session();
        app.push(Role::Event, "new session");
        handle_agent_event(mcp_server_loaded("docs", "docs (3 tools)"), &mut app);
        let texts: Vec<&str> = app
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["new session", "loaded project MCP servers: docs (3 tools)"]
        );
    }

    #[test]
    fn merges_consecutive_reads() {
        let mut app = new_app();
        app.push_tool("read", "a.rs".into(), None);
        app.push_tool("read", "b.rs".into(), None);
        app.push_tool("read", "a.rs".into(), None);
        app.push(Role::Event, "read failed: missing");
        app.push_tool("read", "c.rs".into(), None);
        let texts: Vec<&str> = app.messages[1..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["read a.rs (2), b.rs", "read failed: missing", "read c.rs"]
        );
    }

    #[test]
    fn lists_files_once_in_the_background_and_shows_them_when_ready() {
        let mut app = new_app();
        assert!(app.start_listing_files().is_none());
        handle_input(Event::Paste("read @ma".into()), &mut app, |_| {});
        assert!(app.visible_suggestions().is_none());
        let generation = app.start_listing_files().unwrap();
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Char('i'))),
            &mut app,
            |_| {},
        );
        assert!(app.start_listing_files().is_none());
        handle_agent_event(
            UiEvent::Files(generation, vec!["README.md".into(), "src/main.rs".into()]),
            &mut app,
        );
        let suggestions = app.visible_suggestions().unwrap();
        assert_eq!(suggestions.items, [("src/main.rs".into(), String::new())]);
        assert!(screen(&mut app).contains("src/main.rs"));
        assert!(app.start_listing_files().is_none());
    }

    #[test]
    fn drops_a_file_list_started_before_the_prompt_was_sent() {
        for activity in [None, Some(Activity::Working)] {
            let mut app = new_app();
            app.activity = activity;
            handle_input(Event::Paste("read @".into()), &mut app, |_| {});
            let generation = app.start_listing_files().unwrap();
            handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {});
            handle_agent_event(UiEvent::Files(generation, vec!["old.rs".into()]), &mut app);
            handle_input(Event::Paste("@".into()), &mut app, |_| {});
            assert!(app.start_listing_files().is_some(), "{activity:?}");
            assert!(!screen(&mut app).contains("old.rs"), "{activity:?}");
        }
    }

    #[test]
    fn renders_a_streamed_reply_the_same_as_the_whole_reply() {
        let reply = "Intro with **bold** text.\n\n```rust\nfn main() {}\n```\n\n\
            1. Step one\n   ```sh\n   cargo test\n   ```\n2. Step two\n\n\
            # Heading\n```\nno blank line before or after\n```\nafter\n\
            | a | b |\n| --- | --- |\n| 1 | 2 |\n\n~~~\ntilde\n~~~\n\n\
            > quote\n\n```python\nprint('x')\n```\n";
        let mut streamed = new_app();
        let characters: Vec<char> = reply.chars().collect();
        for chunk in characters.chunks(3) {
            let text: String = chunk.iter().collect();
            handle_agent_event(UiEvent::Agent(AgentEvent::Text(text)), &mut streamed);
            screen(&mut streamed);
        }
        let mut whole = new_app();
        whole.push(Role::Assistant, reply);
        let shown = screen(&mut whole);
        let rendered = |app: &App| {
            let rendered = app.messages.last().unwrap().rendered.as_ref().unwrap();
            (rendered.lines.clone(), rendered.heights.clone())
        };
        assert_eq!(rendered(&streamed), rendered(&whole));
        assert_eq!(screen(&mut streamed), shown);
    }

    #[test]
    fn rerenders_only_width_dependent_messages_on_resize() {
        let messages = [
            (
                Role::Tool,
                "edit src/main.rs\n-fn old() {}\n+fn new() { println!(\"a line long enough to wrap\"); }",
                false,
            ),
            (Role::Event, "an event line long enough to wrap", false),
            (Role::Thinking, "thinking about something long", false),
            (
                Role::Assistant,
                "Some text\n\n```rust\nfn main() { println!(\"hi there\"); }\n```\n",
                false,
            ),
            (Role::User, "a prompt", true),
            (
                Role::Assistant,
                "Table:\n\n| a | b |\n| --- | --- |\n| one two three four | five six seven |\n",
                true,
            ),
            (
                Role::Assistant,
                "> | a | b |\n> |:-:|---|\n> | one two three four | five six seven |\n",
                true,
            ),
        ];
        for (role, text, depends_on_width) in messages {
            let message = || ChatMessage {
                role,
                text: text.into(),
                rendered: None,
            };
            let mut resized = message();
            resized.render(60);
            resized.render(20);
            let mut fresh = message();
            fresh.render(20);
            let (resized, fresh) = (resized.rendered.unwrap(), fresh.rendered.unwrap());
            assert_eq!(resized.lines, fresh.lines, "{text}");
            assert_eq!(resized.heights, fresh.heights, "{text}");
            assert_eq!(resized.height, fresh.height, "{text}");

            let mut marked = message();
            marked.render(60);
            marked.rendered.as_mut().unwrap().lines[0] = Line::raw("cached");
            marked.render(20);
            let rendered = marked.rendered.unwrap();
            let kept = rendered.lines[0] == Line::raw("cached");
            assert_eq!(kept, !depends_on_width, "{text}");
        }
    }
}
