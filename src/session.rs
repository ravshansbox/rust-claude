use std::{
    cmp::Reverse,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config;

mod files;
mod id;
mod images;
mod lines;
mod lock;
mod read;

use files::{latest_in_folder, session_files};
use id::{hex, new_uuid};
use lines::{drop_mismatched_thinking_line, header};
use lock::open_locked;
use read::{SavedSession, read_messages, read_preview};

const EXTENSION: &str = "jsonl";

pub struct SessionSummary {
    pub id: String,
    pub modified: SystemTime,
    pub preview: String,
}

pub struct Session {
    pub id: String,
    saved: usize,
    /// Whether requests ask the API to drop thinking blocks whose earlier
    /// conversation changed, instead of rejecting the request.
    drops_mismatched_thinking: bool,
    /// Whether that choice still needs writing to the session file.
    drops_mismatched_thinking_unsaved: bool,
    /// The session file, locked so that no other rust-coding-agent writes to it.
    file: Option<File>,
}

fn sessions_directory() -> Result<PathBuf> {
    config::file("sessions")
}

fn session_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.{EXTENSION}"))
}

impl Session {
    pub fn new() -> Result<Self> {
        Ok(Self {
            id: new_uuid()?,
            saved: 0,
            drops_mismatched_thinking: false,
            drops_mismatched_thinking_unsaved: false,
            file: None,
        })
    }

    pub fn drops_mismatched_thinking(&self) -> bool {
        self.drops_mismatched_thinking
    }

    /// Makes every later request in this session, including after a resume,
    /// drop thinking blocks whose earlier conversation changed. The choice is
    /// written with the next save.
    pub fn drop_mismatched_thinking(&mut self) {
        if !self.drops_mismatched_thinking {
            self.drops_mismatched_thinking = true;
            self.drops_mismatched_thinking_unsaved = true;
        }
    }

    pub fn save(&mut self, messages: &[Value]) -> Result<()> {
        let new_messages = messages.get(self.saved..).unwrap_or_default();
        if new_messages.is_empty() && !self.drops_mismatched_thinking_unsaved {
            return Ok(());
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => {
                let directory = sessions_directory()?;
                config::create_private_dir(&directory)?;
                let path = session_path(&directory, &self.id);
                self.file.insert(open_locked(&path, true)?)
            }
        };
        let header = (self.saved == 0).then(header);
        let drop_line = self
            .drops_mismatched_thinking_unsaved
            .then(drop_mismatched_thinking_line);
        let mut lines = Vec::new();
        for line in header.iter().chain(new_messages).chain(&drop_line) {
            serde_json::to_writer(&mut lines, line)?;
            lines.push(b'\n');
        }
        let length = file.metadata()?.len();
        config::append_whole(file, length, &lines)?;
        self.saved += new_messages.len();
        self.drops_mismatched_thinking_unsaved = false;
        Ok(())
    }

    pub fn list_except(id: &str) -> Result<Vec<SessionSummary>> {
        let mut summaries: Vec<SessionSummary> = session_files(&sessions_directory()?)?
            .into_iter()
            .filter(|file| file.id != id)
            .filter_map(|file| {
                Some(SessionSummary {
                    preview: read_preview(&file.path).ok()?,
                    id: file.id,
                    modified: file.modified,
                })
            })
            .collect();
        summaries.sort_by_key(|summary| Reverse(summary.modified));
        Ok(summaries)
    }

    pub fn latest_in_current_folder() -> Result<Option<String>> {
        latest_in_folder(&sessions_directory()?, &current_folder())
    }

    /// The id of the saved session `id`, which must be made of hex digits
    /// and dashes, as new ids are.
    pub fn find(id: &str) -> Result<String> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            anyhow::bail!("not a session id: {id}");
        }
        if !session_path(&sessions_directory()?, id).is_file() {
            anyhow::bail!("no session {id}");
        }
        Ok(id.to_string())
    }

    pub fn load(id: &str) -> Result<(Self, Vec<Value>)> {
        let path = session_path(&sessions_directory()?, id);
        let mut file = open_locked(&path, false)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .with_context(|| format!("reading {}", path.display()))?;
        let SavedSession {
            messages,
            length,
            drops_mismatched_thinking,
        } = read_messages(&bytes);
        // Remove what a crash left after the last whole round, so the next
        // save starts on a new line.
        if file.metadata()?.len() > length {
            file.set_len(length)?;
        }
        let session = Self {
            id: id.to_string(),
            saved: messages.len(),
            drops_mismatched_thinking,
            drops_mismatched_thinking_unsaved: false,
            file: Some(file),
        };
        Ok((session, messages))
    }
}

/// The folder rust-coding-agent runs in, as saved in sessions and prompt history.
pub fn current_folder() -> String {
    std::env::current_dir()
        .map(|folder| folder.display().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
