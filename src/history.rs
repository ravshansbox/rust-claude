use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{config, session};

/// How many prompts history.jsonl keeps; older ones are dropped.
const MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "cwd", default)]
    pub folder: String,
    pub prompt: String,
}

impl Entry {
    /// A prompt sent from the current folder.
    pub fn here(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            folder: session::current_folder(),
        }
    }

    fn line(&self) -> Result<String> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

pub fn history_path() -> Option<PathBuf> {
    Some(config::dir()?.join("history.jsonl"))
}

/// Locks the history file against other running copies until the returned
/// file is dropped. The lock is on a file next to it, because trimming
/// replaces history.jsonl itself.
fn lock(path: &Path) -> Result<std::fs::File> {
    config::create_private_dir(path.parent().context("invalid history path")?)?;
    let lock = config::private_file()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("jsonl.lock"))?;
    lock.lock()?;
    Ok(lock)
}

pub fn append(path: &Path, entry: &Entry) -> Result<()> {
    let _lock = lock(path)?;
    let mut file = config::private_file()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    let length = file.metadata()?.len();
    let mut line = entry.line()?;
    // Start on a new line after a line an earlier write cut short.
    if length > 0 {
        let mut last = [0];
        file.seek(SeekFrom::Start(length - 1))?;
        file.read_exact(&mut last)?;
        if last != *b"\n" {
            line.insert(0, '\n');
        }
    }
    if let Err(error) = file.write_all(line.as_bytes()) {
        // Remove a partly written line, so the next prompt starts on a new line.
        let _ = file.set_len(length);
        return Err(error.into());
    }
    Ok(())
}

/// Reads every prompt in the history file, oldest first.
pub fn load(path: &Path) -> Vec<Entry> {
    // Read bytes, not text, so a line cut short inside a character costs
    // only that line.
    std::fs::read(path)
        .unwrap_or_default()
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect()
}

/// Drops all but the newest `MAX_ENTRIES` prompts from the history file when
/// `entries` holds more, and sets `entries` to what the file keeps. It reads
/// the file again so prompts other running copies sent since are kept.
pub fn trim(path: &Path, entries: &mut Vec<Entry>) -> Result<()> {
    if entries.len() <= MAX_ENTRIES {
        return Ok(());
    }
    let _lock = lock(path)?;
    let mut current = load(path);
    current.drain(..current.len().saturating_sub(MAX_ENTRIES));
    let text = current
        .iter()
        .map(Entry::line)
        .collect::<Result<String>>()?;
    config::write_private_file(path, text.as_bytes())?;
    *entries = current;
    Ok(())
}

/// The prompts sent from `folder`, oldest first, each at its newest position.
pub fn folder_prompts(entries: &[Entry], folder: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut prompts: Vec<String> = entries
        .iter()
        .rev()
        .filter(|entry| entry.folder == folder && seen.insert(entry.prompt.as_str()))
        .map(|entry| entry.prompt.clone())
        .collect();
    prompts.reverse();
    prompts
}

/// Every prompt, newest first, each at its newest position.
pub fn newest_unique(entries: &[Entry]) -> Vec<Entry> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .rev()
        .filter(|entry| seen.insert(entry.prompt.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
