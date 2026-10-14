use std::{
    fmt::Write as _,
    fs::{File, TryLockError},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{agent, config, images::Image, skills};

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

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn new_uuid() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("{error}"))?;
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
                let path = directory.join(format!("{}.jsonl", self.id));
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
        if let Err(error) = file.write_all(&lines) {
            // Remove a partly written line, so the next save starts on a new line.
            let _ = file.set_len(length);
            return Err(error.into());
        }
        self.saved += new_messages.len();
        self.drops_mismatched_thinking_unsaved = false;
        Ok(())
    }

    fn images_directory(&self) -> Result<PathBuf> {
        Ok(sessions_directory()?.join(&self.id))
    }

    pub fn save_image(&self, image: &Image) -> Result<Value> {
        let directory = self.images_directory()?;
        config::create_private_dir(&directory)?;
        let hash = hex(&Sha256::digest(&image.data)[..8]);
        let file = format!("{hash}.{}", image.extension());
        let path = directory.join(&file);
        // Images are written whole through a temporary file, so a file of
        // another length is what an older version left cut short.
        let saved = std::fs::metadata(&path)
            .is_ok_and(|metadata| metadata.len() == image.data.len() as u64);
        if !saved {
            config::write_private_file(&path, &image.data)?;
        }
        Ok(json!({ "type": "image", "file": file, "media_type": image.media_type }))
    }

    pub fn inline_images(&self, messages: &mut [Value]) -> Result<()> {
        inline_images(&self.images_directory()?, messages)
    }

    pub fn list_except(id: &str) -> Result<Vec<SessionSummary>> {
        let mut summaries = Vec::new();
        for file in session_files(&sessions_directory()?)? {
            if file.id == id {
                continue;
            }
            let Ok(preview) = read_preview(&file.path) else {
                continue;
            };
            summaries.push(SessionSummary {
                id: file.id,
                modified: file.modified,
                preview,
            });
        }
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.modified));
        Ok(summaries)
    }

    pub fn latest_in_current_folder() -> Result<Option<String>> {
        latest_in_folder(&sessions_directory()?, &current_folder())
    }

    pub fn load(id: &str) -> Result<(Self, Vec<Value>)> {
        let path = sessions_directory()?.join(format!("{id}.jsonl"));
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
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));
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
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
            continue;
        };
        files.push(SessionFile {
            id: id.to_string(),
            path,
            modified,
        });
    }
    Ok(files)
}

fn inline_images(directory: &Path, messages: &mut [Value]) -> Result<()> {
    let blocks = messages
        .iter_mut()
        .filter_map(|message| message["content"].as_array_mut())
        .flatten()
        .filter(|block| block["type"] == "image");
    for block in blocks {
        let Some(object) = block.as_object_mut() else {
            continue;
        };
        let Some(file) = object.remove("file") else {
            continue;
        };
        let file = file.as_str().unwrap_or_default();
        let data = std::fs::read(directory.join(file))
            .with_context(|| format!("failed to read image {file}"))?;
        let media_type = object.remove("media_type").unwrap_or_default();
        object.insert(
            "source".into(),
            json!({ "type": "base64", "media_type": media_type, "data": STANDARD.encode(data) }),
        );
    }
    Ok(())
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

fn read_folder(path: &Path) -> Result<Option<String>> {
    let reader = BufReader::new(File::open(path)?);
    let Some(line) = reader.lines().next().transpose()? else {
        return Ok(None);
    };
    let line: Value = serde_json::from_str(&line)?;
    Ok(line["cwd"]
        .as_str()
        .filter(|_| is_header(&line))
        .map(str::to_string))
}

fn read_preview(path: &Path) -> Result<String> {
    let reader = BufReader::new(File::open(path)?);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        let Some(text) = message["content"].as_str().or_else(|| {
            message["content"]
                .as_array()
                .filter(|_| message["role"] == "user")
                .and_then(|blocks| blocks.iter().find(|block| block["type"] == "text"))
                .and_then(|block| block["text"].as_str())
        }) else {
            continue;
        };
        if let Some((command, _)) = agent::parse_shell_message(text) {
            return Ok(format!("!{command}"));
        }
        if let Some(block) = skills::parse_block(text) {
            return Ok(match block.user_message {
                Some(user_message) => format!("/skill:{} {user_message}", block.name),
                None => format!("/skill:{}", block.name),
            });
        }
        return Ok(text.to_string());
    }
    Ok(String::new())
}

struct SavedSession {
    messages: Vec<Value>,
    /// Length of the file up to the end of the last kept line.
    length: u64,
    drops_mismatched_thinking: bool,
}

/// Reads the messages before the first damaged line, such as a line a crash
/// cut short.
fn read_messages(bytes: &[u8]) -> SavedSession {
    let marker = drop_mismatched_thinking_line();
    let mut messages = Vec::new();
    let mut starts = Vec::new();
    let mut marker_at = None;
    let mut length = 0;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.trim_ascii().is_empty() {
            length += line.len();
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(value) if line.ends_with(b"\n") => {
                if value == marker {
                    marker_at.get_or_insert(length);
                } else if !is_header(&value) {
                    starts.push(length);
                    messages.push(value);
                }
                length += line.len();
            }
            _ => break,
        }
    }
    // The API rejects tool calls without results, which a round cut short
    // leaves, even when the cut falls between lines.
    if messages.last().is_some_and(has_tool_calls) {
        messages.pop();
        length = starts.pop().unwrap_or_default();
    }
    SavedSession {
        messages,
        length: length as u64,
        drops_mismatched_thinking: marker_at.is_some_and(|at| at < length),
    }
}

fn has_tool_calls(message: &Value) -> bool {
    message["role"] == "assistant"
        && message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use"))
}

#[cfg(test)]
mod tests;
