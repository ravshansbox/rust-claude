use super::{
    commands::command_matches,
    display_model,
    files::{file_matches, file_query, list_files},
    render::{render_message, tool_message},
    workspace_label,
};
use crate::{
    agent::{Queue, Queued, Stats, THINKING_LEVELS, take_queued},
    history,
    images::Image,
    skills::Skill,
    tools,
};
use anyhow::Result;
use ratatui::text::Line;
use std::path::PathBuf;

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
    pub(super) rendered: Option<(u16, Vec<Line<'static>>)>,
}

impl ChatMessage {
    pub(super) fn append(&mut self, text: &str) {
        self.text.push_str(text);
        self.rendered = None;
    }

    pub(super) fn render(&mut self, width: u16) {
        if self
            .rendered
            .as_ref()
            .is_none_or(|(rendered_width, _)| *rendered_width != width)
        {
            self.rendered = Some((width, render_message(self.role, &self.text, width)));
        }
    }
}

pub(super) struct App {
    pub(super) input: String,
    pub(super) cursor: usize,
    pub(super) messages: Vec<ChatMessage>,
    pub(super) workspace: String,
    pub(super) model: String,
    pub(super) thinking_level: &'static str,
    pub(super) status: String,
    pub(super) spinner_frame: usize,
    pub(super) stats: Stats,
    pub(super) scroll_from_bottom: u16,
    pub(super) max_scroll: u16,
    pub(super) page_size: u16,
    pub(super) input_width: usize,
    pub(super) busy: bool,
    pub(super) picker: Option<Picker>,
    pub(super) command_selected: usize,
    pub(super) commands_dismissed: bool,
    pub(super) files: Option<Vec<String>>,
    pub(super) prompt_history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) reads: tools::ReadGroup,
    pub(super) skills: Vec<Skill>,
    pub(super) images: Vec<(usize, Image)>,
    pub(super) image_count: usize,
    pub(super) queue: Queue,
    pub(super) history_file: Option<PathBuf>,
    pub(super) history_search: Option<HistorySearch>,
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
    pub(super) fn new(model: &str, thinking_level: &'static str, stats: Stats) -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            messages: Vec::new(),
            workspace: workspace_label(),
            model: model.into(),
            thinking_level,
            status: String::new(),
            spinner_frame: 0,
            stats,
            scroll_from_bottom: 0,
            max_scroll: 0,
            page_size: 1,
            input_width: usize::MAX,
            busy: false,
            picker: None,
            command_selected: 0,
            commands_dismissed: false,
            files: None,
            prompt_history: Vec::new(),
            history_index: None,
            reads: tools::ReadGroup::default(),
            skills: Vec::new(),
            images: Vec::new(),
            image_count: 0,
            queue: Queue::default(),
            history_file: None,
            history_search: None,
        }
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

    pub(super) fn load_history(&mut self, path: Option<PathBuf>) {
        let folder = std::env::current_dir()
            .map(|folder| folder.display().to_string())
            .unwrap_or_default();
        self.prompt_history = path
            .as_deref()
            .map(|path| history::load_folder(path, &folder))
            .unwrap_or_default();
        self.history_file = path;
    }

    pub(super) fn add_prompt(&mut self, prompt: String) {
        self.prompt_history.retain(|existing| *existing != prompt);
        self.prompt_history.push(prompt);
    }

    pub(super) fn remember(&mut self, prompt: &str) {
        self.add_prompt(prompt.to_string());
        let Some(path) = &self.history_file else {
            return;
        };
        if let Err(error) = history::append(path, prompt) {
            self.push(
                Role::Event,
                format!("failed to save prompt history: {error}"),
            );
        }
    }

    pub(super) fn open_history_search(&mut self) {
        let current = self.prompt_history.iter().rev().cloned().collect();
        self.history_search = Some(HistorySearch {
            all: false,
            query: String::new(),
            current,
            everywhere: self
                .history_file
                .as_deref()
                .map(history::load)
                .unwrap_or_default(),
            selected: 0,
        });
    }

    pub(super) fn scroll_up(&mut self, amount: u16) {
        self.scroll_from_bottom = self
            .scroll_from_bottom
            .saturating_add(amount)
            .min(self.max_scroll);
    }

    pub(super) fn scroll_down(&mut self, amount: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(amount);
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.scroll_from_bottom = self.max_scroll;
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }

    pub(super) fn previous_prompt(&mut self) {
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None if self.prompt_history.is_empty() => return,
            None => self.prompt_history.len() - 1,
        };
        self.history_index = Some(index);
        self.input = self.prompt_history[index].clone();
        self.cursor = self.input.len();
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
    }

    pub(super) fn visible_suggestions(&self) -> Option<Suggestions> {
        if self.commands_dismissed {
            return None;
        }
        let commands = command_matches(&self.input, &self.skills);
        if !commands.is_empty() {
            return Some(Suggestions {
                start: 0,
                end: self.input.len(),
                items: commands,
                files: false,
            });
        }
        let (start, query) = file_query(&self.input, self.cursor)?;
        let items: Vec<(String, String)> = file_matches(self.files.as_deref()?, query)
            .into_iter()
            .map(|path| (path, String::new()))
            .collect();
        if items.is_empty() {
            return None;
        }
        Some(Suggestions {
            start,
            end: self.cursor,
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

    pub(super) fn cycle_thinking_level(&mut self) {
        let index = THINKING_LEVELS
            .iter()
            .position(|level| *level == self.thinking_level)
            .map_or(0, |index| (index + 1) % THINKING_LEVELS.len());
        self.thinking_level = THINKING_LEVELS[index];
    }

    pub(super) fn input_changed(&mut self) {
        self.history_index = None;
        self.command_selected = 0;
        self.commands_dismissed = false;
        if self.files.is_none() && file_query(&self.input, self.cursor).is_some() {
            self.files = Some(list_files());
        }
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

    pub(super) fn can_queue(&self) -> bool {
        self.busy && (self.status == "working" || self.status == "compacting")
    }

    pub(super) fn queue_prompt(&mut self) {
        let prompt = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.files = None;
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
        self.start("working");
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

    pub(super) fn start(&mut self, status: &str) {
        self.status = status.into();
        self.busy = true;
    }

    pub(super) fn set_thinking_level(&mut self, name: &str) {
        match THINKING_LEVELS.iter().find(|level| **level == name) {
            Some(level) => {
                self.thinking_level = level;
                self.push(Role::Event, format!("thinking: {level}"));
            }
            None => self.push(
                Role::Event,
                format!(
                    "unknown thinking level: {name} (options: {})",
                    THINKING_LEVELS.join(", ")
                ),
            ),
        }
    }

    pub(super) fn set_model(&mut self, model: &str) {
        self.model = model.into();
        self.push(Role::Event, format!("model: {}", display_model(model)));
    }

    pub(super) fn clear_session(&mut self) {
        self.messages.clear();
        self.history_index = None;
    }

    pub(super) fn finish<T>(&mut self, result: Result<T>, on_success: impl FnOnce(&mut Self, T)) {
        match result {
            Ok(value) => on_success(self, value),
            Err(error) => self.push(Role::Event, format!("error: {error}")),
        }
        self.busy = false;
    }
}

pub(super) fn image_marker(number: usize) -> String {
    format!("[image {number}]")
}

#[cfg(test)]
mod tests {
    use super::Role;
    use crate::tui::test_support::new_app;

    #[test]
    fn merges_consecutive_reads() {
        let mut app = new_app();
        app.push_tool("read", "a.rs".into(), None);
        app.push_tool("read", "b.rs".into(), None);
        app.push_tool("read", "a.rs".into(), None);
        app.push(Role::Event, "read failed: missing");
        app.push_tool("read", "c.rs".into(), None);
        let texts: Vec<&str> = app.messages[..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["read a.rs (2), b.rs", "read failed: missing", "read c.rs"]
        );
    }
}
