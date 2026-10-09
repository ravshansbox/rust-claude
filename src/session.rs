use std::{fs::OpenOptions, io::Write, path::PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

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

    pub fn latest_other(&self) -> Result<Option<(Session, Vec<Value>)>> {
        let directory = sessions_directory()?;
        if !directory.exists() {
            return Ok(None);
        }
        let mut latest = None;
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if id == self.id {
                continue;
            }
            let modified = std::fs::metadata(&path)?.modified()?;
            if latest
                .as_ref()
                .is_none_or(|(latest_modified, _, _)| modified > *latest_modified)
            {
                latest = Some((modified, id.to_string(), path));
            }
        }
        let Some((_, id, path)) = latest else {
            return Ok(None);
        };
        let messages = std::fs::read_to_string(&path)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<Value>, _>>()
            .with_context(|| format!("reading {}", path.display()))?;
        let session = Session {
            id,
            saved: messages.len(),
        };
        Ok(Some((session, messages)))
    }
}

#[cfg(test)]
mod tests {
    use super::new_uuid;

    #[test]
    fn generates_version_4_uuid() {
        let uuid = new_uuid().unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"));
    }
}
