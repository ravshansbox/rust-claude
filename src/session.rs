use std::{
    cmp::Reverse,
    fs::{File, TryLockError},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config;

mod images;
mod read;

use read::{SavedSession, read_folder, read_messages, read_preview};

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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn new_uuid() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
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

/// Opens a session file for appending, locked until it is closed.
fn open_locked(path: &Path, create: bool) -> Result<File> {
    let file = config::private_file()
        .create(create)
        .read(true)
        .append(true)
        .open(path)?;
    // A command started while the file was open shares the lock until the
    // command takes over, so a lock just released can look held for a moment.
    let mut attempts = 0;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if attempts < 20 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                anyhow::bail!("this session is open in another rust-coding-agent")
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn latest_in_folder(directory: &Path, folder: &str) -> Result<Option<String>> {
    let mut files = session_files(directory)?;
    files.sort_by_key(|file| Reverse(file.modified));
    Ok(files
        .into_iter()
        .find(|file| read_folder(&file.path).ok().flatten().as_deref() == Some(folder))
        .map(|file| file.id))
}

struct SessionFile {
    id: String,
    path: PathBuf,
    modified: SystemTime,
}

/// Lists the session files in `directory`, skipping those whose name or
/// change time can't be read.
fn session_files(directory: &Path) -> Result<Vec<SessionFile>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let paths = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(paths
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == EXTENSION)
        })
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?.to_string();
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()?;
            Some(SessionFile { id, path, modified })
        })
        .collect())
}

/// The folder rust-coding-agent runs in, as saved in sessions and prompt history.
pub fn current_folder() -> String {
    std::env::current_dir()
        .map(|folder| folder.display().to_string())
        .unwrap_or_default()
}

fn header() -> Value {
    json!({ "type": "session", "cwd": current_folder() })
}

fn is_header(line: &Value) -> bool {
    line["type"] == "session"
}

/// Records that the session drops thinking blocks whose earlier conversation
/// changed.
fn drop_mismatched_thinking_line() -> Value {
    json!({ "type": "thinking_block_binding", "prefix_mismatch_behavior": "drop_block" })
}

#[cfg(test)]
mod tests;
