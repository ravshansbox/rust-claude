use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use serde_json::Value;

pub struct SessionSummary {
    pub id: String,
    pub modified: SystemTime,
    pub preview: String,
}

pub struct Session {
    pub id: String,
    saved: usize,
}

fn sessions_directory() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".rust-claude").join("sessions"))
}

fn new_uuid() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("{error}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
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
        })
    }

    pub fn save(&mut self, messages: &[Value]) -> Result<()> {
        if messages.len() <= self.saved {
            return Ok(());
        }
        let directory = sessions_directory()?;
        std::fs::create_dir_all(&directory)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join(format!("{}.jsonl", self.id)))?;
        let mut lines = String::new();
        for message in &messages[self.saved..] {
            lines.push_str(&serde_json::to_string(message)?);
            lines.push('\n');
        }
        file.write_all(lines.as_bytes())?;
        self.saved = messages.len();
        Ok(())
    }

    pub fn list_others(&self) -> Result<Vec<SessionSummary>> {
        let directory = sessions_directory()?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut summaries = Vec::new();
        for entry in std::fs::read_dir(&directory)? {
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
            if id == self.id {
                continue;
            }
            let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified())
            else {
                continue;
            };
            let Ok(messages) = read_messages(&path) else {
                continue;
            };
            let preview = messages
                .iter()
                .find_map(|message| message["content"].as_str().map(str::to_string))
                .unwrap_or_default();
            summaries.push(SessionSummary {
                id: id.to_string(),
                modified,
                preview,
            });
        }
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.modified));
        Ok(summaries)
    }

    pub fn load(id: &str) -> Result<(Session, Vec<Value>)> {
        let messages = read_messages(&sessions_directory()?.join(format!("{id}.jsonl")))?;
        let session = Session {
            id: id.to_string(),
            saved: messages.len(),
        };
        Ok((session, messages))
    }
}

fn read_messages(path: &Path) -> Result<Vec<Value>> {
    std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()
        .with_context(|| format!("reading {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{Session, new_uuid, sessions_directory};

    #[test]
    fn generates_version_4_uuid() {
        let uuid = new_uuid().unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn does_not_create_file_without_messages() {
        let mut session = Session::new().unwrap();
        session.save(&[]).unwrap();
        let path = sessions_directory()
            .unwrap()
            .join(format!("{}.jsonl", session.id));
        assert!(!path.exists());
    }

    #[test]
    fn skips_unreadable_session_files() {
        let session = Session::new().unwrap();
        let directory = sessions_directory().unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{}.jsonl", new_uuid().unwrap()));
        std::fs::write(&path, "not json\n").unwrap();
        let result = session.list_others();
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_ok());
    }
}
