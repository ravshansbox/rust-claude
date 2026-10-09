use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub struct Entry {
    pub prompt: String,
    pub folder: String,
}

pub fn history_path() -> Option<PathBuf> {
    Some(crate::config::dir()?.join("history.jsonl"))
}

pub fn append(path: &Path, prompt: &str) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("invalid history path")?)?;
    let folder = std::env::current_dir()
        .map(|folder| folder.display().to_string())
        .unwrap_or_default();
    let mut line = serde_json::to_string(&json!({ "prompt": prompt, "cwd": folder }))?;
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())?;
    Ok(())
}

pub fn load_folder(path: &Path, folder: &str) -> Vec<String> {
    let mut prompts: Vec<String> = load_all(path)
        .into_iter()
        .filter(|entry| entry.folder == folder)
        .map(|entry| entry.prompt)
        .collect();
    let mut seen = std::collections::HashSet::new();
    prompts.retain(|prompt| seen.insert(prompt.clone()));
    prompts.reverse();
    prompts
}

pub fn load(path: &Path) -> Vec<Entry> {
    let mut entries = load_all(path);
    let mut seen = std::collections::HashSet::new();
    entries.retain(|entry| seen.insert(entry.prompt.clone()));
    entries
}

fn load_all(path: &Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
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

#[cfg(test)]
mod tests {
    use super::{append, load};

    #[test]
    fn loads_newest_first_without_repeats() {
        let path = std::env::temp_dir().join(format!(
            "rust-claude-history-{}/history.jsonl",
            std::process::id()
        ));
        for prompt in ["one", "two", "one", "three"] {
            append(&path, prompt).unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"not json\n"))
            .unwrap();
        let entries = load(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let prompts: Vec<&str> = entries.iter().map(|entry| entry.prompt.as_str()).collect();
        assert_eq!(prompts, ["three", "one", "two"]);
        let folder = std::env::current_dir().unwrap().display().to_string();
        assert!(entries.iter().all(|entry| entry.folder == folder));
    }
}
