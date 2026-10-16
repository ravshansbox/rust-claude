use super::{
    App,
    commands::command_matches,
    files::{file_query, path_query},
};
use std::borrow::Cow;

pub(super) struct Suggestions {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) items: Vec<(String, String)>,
    pub(super) files: bool,
}

impl App {
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
        let paths = match path_query(query) {
            Some((folder, name)) => self.folder.as_ref()?.matches(folder, name)?,
            None => self.files.as_ref()?.matches(query),
        };
        let items: Vec<(String, String)> = paths
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
        let folder = suggestions.files && name.ends_with('/');
        let replacement = if folder {
            Cow::Owned(format!("@{name}"))
        } else if suggestions.files {
            Cow::Owned(format!("@{name} "))
        } else {
            Cow::Borrowed(name.as_str())
        };
        let mut end = suggestions.end;
        if suggestions.files && !folder && self.input[end..].starts_with(' ') {
            end += 1;
        }
        self.input
            .replace_range(suggestions.start..end, &replacement);
        self.cursor = suggestions.start + replacement.len();
    }
}
