use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use anyhow::Result;
use serde_json::Value;

use super::lines::{drop_mismatched_thinking_line, is_header};
use crate::{agent, skills};

pub(super) fn read_folder(path: &Path) -> Result<Option<String>> {
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

pub(super) fn read_preview(path: &Path) -> Result<String> {
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
            return Ok(block.command());
        }
        return Ok(text.to_string());
    }
    Ok(String::new())
}

pub(super) struct SavedSession {
    pub(super) messages: Vec<Value>,
    /// Length of the file up to the end of the last kept line.
    pub(super) length: u64,
    pub(super) drops_mismatched_thinking: bool,
}

/// Reads the messages before the first damaged line, such as a line a crash
/// cut short.
pub(super) fn read_messages(bytes: &[u8]) -> SavedSession {
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
        length: u64::try_from(length).unwrap_or(u64::MAX),
        drops_mismatched_thinking: marker_at.is_some_and(|at| at < length),
    }
}

fn has_tool_calls(message: &Value) -> bool {
    message["role"] == "assistant"
        && message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use"))
}
