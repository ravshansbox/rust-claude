use std::{
    fs::{File, TryLockError},
    io::{Read, Write},
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
    /// Whether requests ask the API to drop thinking blocks whose earlier
    /// conversation changed, instead of rejecting the request.
    drops_mismatched_thinking: bool,
    /// Whether that choice still needs writing to the session file.
    drops_mismatched_thinking_unsaved: bool,
    /// The session file, locked so that no other rust-claude writes to it.
    file: Option<File>,
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
                crate::config::create_private_dir(&directory)?;
                let path = directory.join(format!("{}.jsonl", self.id));
                self.file.insert(open_locked(&path, true)?)
            }
        };
        let mut lines = String::new();
        if self.saved == 0 {
            lines.push_str(&serde_json::to_string(&header())?);
            lines.push('\n');
        }
        for message in new_messages {
            lines.push_str(&serde_json::to_string(message)?);
            lines.push('\n');
        }
        if self.drops_mismatched_thinking_unsaved {
            lines.push_str(&serde_json::to_string(&drop_mismatched_thinking_line())?);
            lines.push('\n');
        }
        let length = file.metadata()?.len();
        if let Err(error) = file.write_all(lines.as_bytes()) {
            // Remove a partly written line, so the next save starts on a new line.
            let _ = file.set_len(length);
            return Err(error.into());
        }
        self.saved += new_messages.len();
        self.drops_mismatched_thinking_unsaved = false;
        Ok(())
    }

    pub fn save_image(&self, image: &Image) -> Result<Value> {
        let directory = sessions_directory()?.join(&self.id);
        crate::config::create_private_dir(&directory)?;
        let hash: String = Sha256::digest(&image.data)[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let file = format!("{hash}.{}", image.extension());
        let path = directory.join(&file);
        // Images are written whole through a temporary file, so a file of
        // another length is what an older version left cut short.
        let saved = std::fs::metadata(&path)
            .is_ok_and(|metadata| metadata.len() == image.data.len() as u64);
        if !saved {
            crate::config::write_private_file(&path, &image.data)?;
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
        latest_in_folder(&sessions_directory()?, &current_folder())
    }

    pub fn load(id: &str) -> Result<(Session, Vec<Value>)> {
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
        let session = Session {
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
    let file = crate::config::private_file()
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
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                anyhow::bail!("this session is open in another rust-claude")
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn latest_in_folder(directory: &Path, folder: &str) -> Result<Option<String>> {
    if !directory.exists() {
        return Ok(None);
    }
    let mut latest: Option<(SystemTime, String)> = None;
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
        if latest
            .as_ref()
            .is_some_and(|(latest_modified, _)| *latest_modified >= modified)
        {
            continue;
        }
        if read_folder(&path).ok().flatten().as_deref() == Some(folder) {
            latest = Some((modified, id.to_string()));
        }
    }
    Ok(latest.map(|(_, id)| id))
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

/// Records that the session drops thinking blocks whose earlier conversation
/// changed.
fn drop_mismatched_thinking_line() -> Value {
    json!({ "type": "thinking_block_binding", "prefix_mismatch_behavior": "drop_block" })
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
    let mut damaged = false;
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
            _ => {
                damaged = true;
                break;
            }
        }
    }
    // The API rejects tool calls without results, which a round cut short leaves.
    if damaged && messages.last().is_some_and(has_tool_calls) {
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
mod tests {
    use super::{
        Session, current_folder, inline_images, latest_in_folder, new_uuid, read_folder,
        read_preview, sessions_directory,
    };
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
        let directory =
            std::env::temp_dir().join(format!("rust-claude-latest-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let now = std::time::SystemTime::now();
        for (id, folder, age) in [
            ("old", "/project", 60),
            ("new", "/project", 30),
            ("other", "/other", 0),
        ] {
            let path = directory.join(format!("{id}.jsonl"));
            std::fs::write(
                &path,
                format!("{}\n", json!({ "type": "session", "cwd": folder })),
            )
            .unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(now - std::time::Duration::from_secs(age)))
                .unwrap();
        }
        let latest = latest_in_folder(&directory, "/project");
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(latest.unwrap().as_deref(), Some("new"));
    }

    /// Saves the messages to a new session and closes it, returning its id
    /// and file.
    fn save_new(messages: &[serde_json::Value]) -> (String, std::path::PathBuf) {
        let mut session = Session::new().unwrap();
        session.save(messages).unwrap();
        let path = sessions_directory()
            .unwrap()
            .join(format!("{}.jsonl", session.id));
        (session.id.clone(), path)
    }

    #[test]
    fn saves_session_folder_and_messages() {
        let messages = vec![json!({ "role": "user", "content": "hello" })];
        let (id, path) = save_new(&messages);
        let loaded = Session::load(&id);
        let folder = read_folder(&path);
        std::fs::remove_file(&path).unwrap();
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        assert!(!path.starts_with(home.join(".rust-claude")));
        assert_eq!(loaded.unwrap().1, messages);
        assert_eq!(folder.unwrap(), Some(current_folder()));
    }

    #[test]
    fn refuses_a_session_open_elsewhere() {
        let messages = vec![json!({ "role": "user", "content": "hello" })];
        let mut session = Session::new().unwrap();
        session.save(&messages).unwrap();
        let while_saving = Session::load(&session.id).map(|_| ());
        let id = session.id.clone();
        drop(session);
        let first = Session::load(&id);
        let second = Session::load(&id).map(|_| ());
        drop(first);
        let after_closing = Session::load(&id).map(|(_, loaded)| loaded);
        std::fs::remove_file(sessions_directory().unwrap().join(format!("{id}.jsonl"))).unwrap();
        for refused in [while_saving, second] {
            assert_eq!(
                refused.unwrap_err().to_string(),
                "this session is open in another rust-claude"
            );
        }
        assert_eq!(after_closing.unwrap(), messages);
    }

    fn append_to_file(path: &std::path::Path, bytes: &[u8]) {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
            .unwrap();
    }

    #[test]
    fn resumes_messages_saved_before_a_line_cut_short() {
        let messages = vec![
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] }),
        ];
        let (id, path) = save_new(&messages);
        // A line cut short in the middle of the two bytes of "é".
        append_to_file(&path, b"{\"role\":\"user\",\"content\":\"caf\xc3");
        let mut more = messages.clone();
        more.push(json!({ "role": "user", "content": "again" }));
        let resumed = Session::load(&id).and_then(|(mut resumed, loaded)| {
            resumed.save(&more)?;
            Ok(loaded)
        });
        let reloaded = Session::load(&id);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(resumed.unwrap(), messages);
        assert_eq!(reloaded.unwrap().1, more);
    }

    #[test]
    fn drops_tool_call_whose_results_were_cut_short() {
        let messages = vec![
            json!({ "role": "user", "content": "run it" }),
            json!({ "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "bash", "input": {} },
            ] }),
        ];
        let (id, path) = save_new(&messages);
        append_to_file(
            &path,
            b"{\"role\":\"user\",\"content\":[{\"type\":\"tool_res",
        );
        let loaded = Session::load(&id);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded.unwrap().1, messages[..1]);
    }

    #[cfg(unix)]
    #[test]
    fn saves_session_and_images_only_the_user_can_read() {
        use crate::config::permissions;
        let mut session = Session::new().unwrap();
        session
            .save(&[json!({ "role": "user", "content": "secret" })])
            .unwrap();
        let image = session.save_image(&crate::images::Image {
            media_type: "image/png",
            data: b"png".to_vec(),
        });
        let file = sessions_directory()
            .unwrap()
            .join(format!("{}.jsonl", session.id));
        let images = sessions_directory().unwrap().join(&session.id);
        let image_file = image.map(|block| images.join(block["file"].as_str().unwrap()));
        let found = (
            permissions(&file),
            permissions(&images),
            permissions(&image_file.unwrap()),
        );
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir_all(&images).unwrap();
        assert_eq!(found, (Some(0o600), Some(0o700), Some(0o600)));
    }

    #[test]
    fn replaces_an_image_a_failed_save_cut_short() {
        let session = Session::new().unwrap();
        let image = crate::images::Image {
            media_type: "image/png",
            data: b"png".to_vec(),
        };
        let block = session.save_image(&image).unwrap();
        let images = sessions_directory().unwrap().join(&session.id);
        std::fs::write(images.join(block["file"].as_str().unwrap()), b"pn").unwrap();
        let mut messages = vec![json!({
            "role": "user",
            "content": [session.save_image(&image).unwrap()],
        })];
        let result = session.inline_images(&mut messages);
        std::fs::remove_dir_all(&images).unwrap();
        result.unwrap();
        assert_eq!(messages[0]["content"][0]["source"]["data"], "cG5n");
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
        let mut readable = Session::new().unwrap();
        readable
            .save(&[json!({ "role": "user", "content": "hello" })])
            .unwrap();
        let directory = sessions_directory().unwrap();
        let unreadable = new_uuid().unwrap();
        let path = directory.join(format!("{unreadable}.jsonl"));
        std::fs::write(&path, "not json\n").unwrap();
        let listed = session.list_others();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(directory.join(format!("{}.jsonl", readable.id))).unwrap();
        let ids: Vec<String> = listed
            .unwrap()
            .into_iter()
            .map(|summary| summary.id)
            .collect();
        assert!(ids.contains(&readable.id));
        assert!(!ids.contains(&unreadable));
    }
}
