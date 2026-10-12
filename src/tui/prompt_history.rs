use super::{App, message::Role};
use crate::{history, session};
use std::path::PathBuf;

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
        let matches = |(prompt, _): &(&str, Option<&str>)| prompt.to_lowercase().contains(&query);
        if self.all {
            self.everywhere
                .iter()
                .map(|entry| (entry.prompt.as_str(), Some(entry.folder.as_str())))
                .filter(matches)
                .collect()
        } else {
            self.current
                .iter()
                .map(|prompt| (prompt.as_str(), None))
                .filter(matches)
                .collect()
        }
    }
}

impl App {
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
        self.prompt_history = history::folder_prompts(&self.history, &session::current_folder());
        self.history_file = path;
    }

    pub(super) fn add_prompt(&mut self, prompt: String) {
        self.prompt_history.retain(|existing| *existing != prompt);
        self.prompt_history.push(prompt);
    }

    pub(super) fn remember(&mut self, prompt: &str) {
        self.add_prompt(prompt.to_string());
        let entry = history::Entry::here(prompt);
        if let Some(path) = &self.history_file {
            self.unsaved_history.push((path.clone(), entry.clone()));
        }
        self.history.push(entry);
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
        self.input.clone_from(&self.prompt_history[index]);
        self.cursor = self.input.len();
        self.commands_dismissed = true;
    }

    pub(super) fn next_prompt(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.prompt_history.len() {
            self.history_index = Some(index + 1);
            self.input.clone_from(&self.prompt_history[index + 1]);
        } else {
            self.history_index = None;
            self.input.clear();
        }
        self.cursor = self.input.len();
        self.commands_dismissed = self.history_index.is_some();
    }
}

#[cfg(test)]
mod tests;
