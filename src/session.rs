use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::images::Image;

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
    Ok(crate::config::dir()
        .context("HOME is not set")?
        .join("sessions"))
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
        if self.saved == 0 {
            lines.push_str(&serde_json::to_string(&header())?);
            lines.push('\n');
        }
        for message in &messages[self.saved..] {
            lines.push_str(&serde_json::to_string(message)?);
            lines.push('\n');
        }
        file.write_all(lines.as_bytes())?;
        self.saved = messages.len();
        Ok(())
    }

    pub fn save_image(&self, image: &Image) -> Result<Value> {
        let directory = sessions_directory()?.join(&self.id);
        std::fs::create_dir_all(&directory)?;
        let hash: String = Sha256::digest(&image.data)[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let file = format!("{hash}.{}", image.extension());
        let path = directory.join(&file);
        if !path.exists() {
            std::fs::write(&path, &image.data)?;
        }
        Ok(json!({ "type": "image", "file": file, "media_type": image.media_type }))
    }

    pub fn inline_images(&self, messages: &mut [Value]) -> Result<()> {
        let directory = sessions_directory()?.join(&self.id);
        inline_images(&directory, messages)
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
            let Ok(preview) = read_preview(&path) else {
                continue;
            };
            summaries.push(SessionSummary {
                id: id.to_string(),
                modified,
                preview,
            });
        }
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.modified));
        Ok(summaries)
    }

    pub fn latest_in_current_folder() -> Result<Option<String>> {
        let directory = sessions_directory()?;
        if !directory.exists() {
            return Ok(None);
        }
        let folder = current_folder();
        let mut latest: Option<(SystemTime, String)> = None;
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
            let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified())
            else {
                continue;
            };
            if latest
                .as_ref()
                .is_some_and(|(latest_modified, _)| *latest_modified >= modified)
            {
                continue;
            }
            if read_folder(&path).ok().flatten().as_deref() == Some(folder.as_str()) {
                latest = Some((modified, id.to_string()));
            }
        }
        Ok(latest.map(|(_, id)| id))
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

fn current_folder() -> String {
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

fn read_folder(path: &Path) -> Result<Option<String>> {
    let reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let Some(line) = std::io::BufRead::lines(reader).next().transpose()? else {
        return Ok(None);
    };
    let line: Value = serde_json::from_str(&line)?;
    Ok(is_header(&line)
        .then(|| line["cwd"].as_str().map(str::to_string))
        .flatten())
}

fn read_preview(path: &Path) -> Result<String> {
    let reader = std::io::BufReader::new(std::fs::File::open(path)?);
    for line in std::io::BufRead::lines(reader) {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        if let Some(text) = message["content"].as_str() {
            return Ok(text.to_string());
        }
        let text = message["content"]
            .as_array()
            .filter(|_| message["role"] == "user")
            .and_then(|blocks| blocks.iter().find(|block| block["type"] == "text"))
            .and_then(|block| block["text"].as_str());
        if let Some(text) = text {
            return Ok(text.to_string());
        }
    }
    Ok(String::new())
}

fn read_messages(path: &Path) -> Result<Vec<Value>> {
    std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()
        .map(|lines| lines.into_iter().filter(|line| !is_header(line)).collect())
        .with_context(|| format!("reading {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{Session, inline_images, new_uuid, read_preview, sessions_directory};
    use serde_json::json;

    #[test]
    fn inlines_saved_images_as_base64() {
        let directory =
            std::env::temp_dir().join(format!("rust-claude-images-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("a.png"), b"abc").unwrap();
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                { "type": "image", "file": "a.png", "media_type": "image/png" },
                { "type": "text", "text": "look" },
            ],
        })];
        let result = inline_images(&directory, &mut messages);
        std::fs::remove_dir_all(&directory).unwrap();
        result.unwrap();
        assert_eq!(
            messages[0]["content"][0],
            json!({
                "type": "image",
                "source": { "type": "base64", "media_type": "image/png", "data": "YWJj" },
            })
        );
    }

    #[test]
    fn previews_prompt_with_images() {
        let path =
            std::env::temp_dir().join(format!("rust-claude-image-preview-{}", std::process::id()));
        std::fs::write(
            &path,
            "{\"role\":\"user\",\"content\":[{\"type\":\"image\"},{\"type\":\"text\",\"text\":\"look\"}]}\n",
        )
        .unwrap();
        let preview = read_preview(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(preview.unwrap(), "look");
    }

    #[test]
    fn reads_preview_without_reading_rest_of_file() {
        let path = std::env::temp_dir().join(format!("rust-claude-preview-{}", std::process::id()));
        std::fs::write(
            &path,
            "{\"role\":\"user\",\"content\":\"first\"}\nnot json\n",
        )
        .unwrap();
        let preview = read_preview(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(preview.unwrap(), "first");
    }

    #[test]
    fn continues_latest_session_in_current_folder() {
        let mut session = Session::new().unwrap();
        let messages = vec![json!({ "role": "user", "content": "hello" })];
        session.save(&messages).unwrap();
        let latest = Session::latest_in_current_folder();
        let loaded = Session::load(&session.id);
        let path = sessions_directory()
            .unwrap()
            .join(format!("{}.jsonl", session.id));
        let first_line = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(latest.unwrap(), Some(session.id.clone()));
        assert_eq!(loaded.unwrap().1, messages);
        assert!(first_line.starts_with("{\"cwd\":"));
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
