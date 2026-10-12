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
mod tests {
    use super::{Entry, MAX_ENTRIES, append, load, newest_unique, trim};

    fn history_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "rust-coding-agent-{name}history-{}",
                std::process::id()
            ))
            .join("history.jsonl")
    }

    fn write_full_history(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = (0..MAX_ENTRIES + 5)
            .map(|index| format!("{{\"prompt\":\"p{index}\",\"cwd\":\"/project\"}}\n"))
            .collect();
        std::fs::write(path, text).unwrap();
    }

    fn append_raw(path: &std::path::Path, bytes: &[u8]) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
    }

    fn prompts(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.prompt.as_str()).collect()
    }

    #[test]
    fn loads_newest_first_without_repeats() {
        let path = history_file("");
        for prompt in ["one", "two", "one", "three"] {
            append(&path, &Entry::here(prompt)).unwrap();
        }
        append_raw(&path, b"not json\n{\"prompt\":\"x\",\"cwd\":1}\n");
        let (text, entries) = (
            std::fs::read_to_string(&path).unwrap(),
            newest_unique(&load(&path)),
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(prompts(&entries), ["three", "one", "two"]);
        let folder = std::env::current_dir().unwrap().display().to_string();
        assert!(text.starts_with(&format!("{{\"cwd\":{folder:?},\"prompt\":\"one\"}}\n")));
        assert!(entries.iter().all(|entry| entry.folder == folder));
    }

    #[test]
    fn keeps_history_after_a_line_cut_short_inside_a_character() {
        let path = history_file("cut-");
        append(&path, &Entry::here("one")).unwrap();
        append_raw(&path, b"{\"prompt\":\"caf\xc3\n");
        append(&path, &Entry::here("two")).unwrap();
        let entries = load(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(prompts(&entries), ["one", "two"]);
    }

    #[test]
    fn keeps_a_prompt_sent_after_a_line_cut_short_before_its_end() {
        let path = history_file("unended-");
        append(&path, &Entry::here("one")).unwrap();
        append_raw(&path, b"{\"prompt\":\"cut");
        append(&path, &Entry::here("two")).unwrap();
        let entries = load(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(prompts(&entries), ["one", "two"]);
    }

    #[test]
    fn keeps_only_the_newest_prompts() {
        let path = history_file("long-");
        write_full_history(&path);
        let mut entries = load(&path);
        let trimmed = trim(&path, &mut entries);
        let reloaded = load(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        trimmed.unwrap();
        for entries in [entries, reloaded] {
            assert_eq!(entries.len(), MAX_ENTRIES);
            assert_eq!(entries[0].prompt, "p5");
            assert_eq!(entries[0].folder, "/project");
            assert_eq!(
                entries[MAX_ENTRIES - 1].prompt,
                format!("p{}", MAX_ENTRIES + 4)
            );
        }
    }

    #[test]
    fn keeps_prompts_another_copy_sent_after_loading() {
        let path = history_file("shared-");
        write_full_history(&path);
        let mut entries = load(&path);
        // Another running copy sends a prompt after this one loaded.
        append(&path, &Entry::here("from other copy")).unwrap();
        let trimmed = trim(&path, &mut entries);
        let reloaded = load(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        trimmed.unwrap();
        for entries in [entries, reloaded] {
            assert_eq!(entries.len(), MAX_ENTRIES);
            assert_eq!(entries[0].prompt, "p6");
            assert_eq!(entries[MAX_ENTRIES - 1].prompt, "from other copy");
        }
    }

    #[cfg(unix)]
    #[test]
    fn saves_history_only_the_user_can_read() {
        use crate::config::permissions;
        let directory = std::env::temp_dir().join(format!(
            "rust-coding-agent-private-history-{}/config",
            std::process::id()
        ));
        let path = directory.join("history.jsonl");
        append(&path, &Entry::here("secret")).unwrap();
        let found = (permissions(&directory), permissions(&path));
        std::fs::remove_dir_all(directory.parent().unwrap()).unwrap();
        assert_eq!(found, (Some(0o700), Some(0o600)));
    }
}
