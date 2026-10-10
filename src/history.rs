use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// How many prompts history.jsonl keeps; older ones are dropped.
const MAX_ENTRIES: usize = 10_000;

#[derive(Clone)]
pub struct Entry {
    pub prompt: String,
    pub folder: String,
}

impl Entry {
    /// A prompt sent from the current folder.
    pub fn here(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            folder: crate::session::current_folder(),
        }
    }

    fn line(&self) -> Result<String> {
        let mut line =
            serde_json::to_string(&json!({ "prompt": self.prompt, "cwd": self.folder }))?;
        line.push('\n');
        Ok(line)
    }
}

pub fn history_path() -> Option<PathBuf> {
    Some(crate::config::dir()?.join("history.jsonl"))
}

/// Locks the history file against other running copies until the returned
/// file is dropped. The lock is on a file next to it, because trimming
/// replaces history.jsonl itself.
fn lock(path: &Path) -> Result<std::fs::File> {
    crate::config::create_private_dir(path.parent().context("invalid history path")?)?;
    let lock = crate::config::private_file()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("jsonl.lock"))?;
    lock.lock()?;
    Ok(lock)
}

pub fn append(path: &Path, entry: &Entry) -> Result<()> {
    let _lock = lock(path)?;
    crate::config::private_file()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(entry.line()?.as_bytes())?;
    Ok(())
}

/// Reads every prompt in the history file, oldest first.
pub fn load(path: &Path) -> Vec<Entry> {
    // Read bytes, not text, so a line cut short inside a character costs
    // only that line.
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut entries: Vec<Entry> = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(prompt) = value["prompt"].as_str() else {
            continue;
        };
        entries.push(Entry {
            prompt: prompt.to_string(),
            folder: value["cwd"].as_str().unwrap_or_default().to_string(),
        });
    }
    entries
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
    let mut text = String::new();
    for entry in current.iter() {
        text.push_str(&entry.line()?);
    }
    crate::config::write_private_file(path, text.as_bytes())?;
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

    fn prompts(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.prompt.as_str()).collect()
    }

    #[test]
    fn loads_newest_first_without_repeats() {
        let path = std::env::temp_dir().join(format!(
            "rust-claude-history-{}/history.jsonl",
            std::process::id()
        ));
        for prompt in ["one", "two", "one", "three"] {
            append(&path, &Entry::here(prompt)).unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"not json\n"))
            .unwrap();
        let entries = newest_unique(&load(&path));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(prompts(&entries), ["three", "one", "two"]);
        let folder = std::env::current_dir().unwrap().display().to_string();
        assert!(entries.iter().all(|entry| entry.folder == folder));
    }

    #[test]
    fn keeps_history_after_a_line_cut_short_inside_a_character() {
        let path = std::env::temp_dir().join(format!(
            "rust-claude-cut-history-{}/history.jsonl",
            std::process::id()
        ));
        append(&path, &Entry::here("one")).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"{\"prompt\":\"caf\xc3\n"))
            .unwrap();
        append(&path, &Entry::here("two")).unwrap();
        let entries = load(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(prompts(&entries), ["one", "two"]);
    }

    #[test]
    fn keeps_only_the_newest_prompts() {
        let directory =
            std::env::temp_dir().join(format!("rust-claude-long-history-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("history.jsonl");
        let text: String = (0..MAX_ENTRIES + 5)
            .map(|index| format!("{{\"prompt\":\"p{index}\",\"cwd\":\"/project\"}}\n"))
            .collect();
        std::fs::write(&path, text).unwrap();
        let mut entries = load(&path);
        let trimmed = trim(&path, &mut entries);
        let reloaded = load(&path);
        std::fs::remove_dir_all(&directory).unwrap();
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
        let directory =
            std::env::temp_dir().join(format!("rust-claude-shared-history-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("history.jsonl");
        let text: String = (0..MAX_ENTRIES + 5)
            .map(|index| format!("{{\"prompt\":\"p{index}\",\"cwd\":\"/project\"}}\n"))
            .collect();
        std::fs::write(&path, text).unwrap();
        let mut entries = load(&path);
        // Another running copy sends a prompt after this one loaded.
        append(&path, &Entry::here("from other copy")).unwrap();
        let trimmed = trim(&path, &mut entries);
        let reloaded = load(&path);
        std::fs::remove_dir_all(&directory).unwrap();
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
            "rust-claude-private-history-{}/config",
            std::process::id()
        ));
        let path = directory.join("history.jsonl");
        append(&path, &Entry::here("secret")).unwrap();
        let found = (permissions(&directory), permissions(&path));
        std::fs::remove_dir_all(directory.parent().unwrap()).unwrap();
        assert_eq!(found, (Some(0o700), Some(0o600)));
    }
}
