use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, ErrorKind},
};

use serde_json::{Value, json};

use crate::config;

pub mod agent;
pub mod background;
mod bash;
mod display;
mod edit;
mod read;
#[cfg(test)]
pub(crate) mod test_support;
pub mod web_search;
mod write;

pub use bash::{ProcessGroup, new_session};
pub use display::{ReadGroup, diff, note, summary};

const MAX_OUTPUT: usize = 20_000;

pub fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        text.truncate(text.floor_char_boundary(MAX_OUTPUT));
        text.push_str("\n… output truncated");
    }
    text
}

pub fn definitions() -> Value {
    json!([
        bash::definition(),
        read::definition(),
        write::definition(),
        edit::definition(),
        agent::definition(),
        background::stop_definition(),
        web_search::definition()
    ])
}

fn tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": { "type": "object", "properties": properties, "required": required }
    })
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    }
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
}

fn number(input: &Value, key: &str, min: u64) -> Result<Option<u64>, String> {
    match &input[key] {
        Value::Null => Ok(None),
        value => value
            .as_u64()
            .filter(|number| *number >= min)
            .map(Some)
            .ok_or_else(|| format!("{key} must be at least 1")),
    }
}

async fn regular_file(path: &str) -> Result<Metadata, String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("failed to read {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    Ok(metadata)
}

/// Replaces the file at `path`, or the file a symlink there points to, in one
/// step, keeping the permissions and group of the file it replaces. In a
/// folder where no temporary file can be created, and for a file with other
/// hard links or another owner, it writes the file in place instead. With
/// `expected`, it refuses a file whose text is no longer `expected`, such as
/// one another edit changed after this one read it.
async fn replace_file(path: &str, contents: String, expected: Option<String>) -> io::Result<()> {
    let path = std::path::PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        let (target, metadata) = match std::fs::canonicalize(&path) {
            Ok(target) => {
                // Refuses a file the user made read-only, as writing in place would.
                let metadata = OpenOptions::new().write(true).open(&target)?.metadata()?;
                if !metadata.is_file() {
                    return Err(io::Error::other("not a regular file"));
                }
                if let Some(expected) = &expected
                    && std::fs::read(&target)? != expected.as_bytes()
                {
                    return Err(io::Error::other("it changed while being edited"));
                }
                (target, Some(metadata))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                (config::dangling_target(path), None)
            }
            Err(error) => return Err(error),
        };
        let mut options = OpenOptions::new();
        #[cfg(unix)]
        if let Some(metadata) = &metadata {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            // A new file would split the hard links, or belong to this user.
            // SAFETY: geteuid has no preconditions.
            if metadata.nlink() > 1 || metadata.uid() != unsafe { libc::geteuid() } {
                return std::fs::write(&target, contents);
            }
            options.mode(metadata.mode() & 0o777);
        }
        let prepare = |file: &File| {
            let Some(metadata) = &metadata else {
                return Ok(());
            };
            // Keeps the group; a group the user is not in is refused, so the
            // file is written in place.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if file.metadata()?.gid() != metadata.gid() {
                    std::os::unix::fs::fchown(file, None, Some(metadata.gid()))?;
                }
            }
            file.set_permissions(metadata.permissions())
        };
        match config::replace_file(&target, contents.as_bytes(), options, prepare) {
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                std::fs::write(&target, contents)
            }
            result => result,
        }
    })
    .await
    .map_err(io::Error::other)?
}

pub async fn call(name: &str, input: &Value) -> Result<String, String> {
    match name {
        "bash" => bash::run(input).await,
        "read" => read::run(input).await,
        "write" => write::run(input).await,
        "edit" => edit::run(input).await,
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests;
