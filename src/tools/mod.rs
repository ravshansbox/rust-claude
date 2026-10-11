use serde_json::{Value, json};

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

/// Replaces the file at `path`, or the file a symlink there points to, in one
/// step, keeping the permissions and group of the file it replaces. In a
/// folder where no temporary file can be created, and for a file with other
/// hard links or another owner, it writes the file in place instead. With
/// `expected`, it refuses a file whose text is no longer `expected`, such as
/// one another edit changed after this one read it.
async fn replace_file(
    path: &str,
    contents: String,
    expected: Option<String>,
) -> std::io::Result<()> {
    let path = std::path::PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        let (target, metadata) = match std::fs::canonicalize(&path) {
            Ok(target) => {
                // Refuses a file the user made read-only, as writing in place would.
                let metadata = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&target)?
                    .metadata()?;
                if !metadata.is_file() {
                    return Err(std::io::Error::other("not a regular file"));
                }
                if let Some(expected) = &expected
                    && std::fs::read(&target)? != expected.as_bytes()
                {
                    return Err(std::io::Error::other("it changed while being edited"));
                }
                (target, Some(metadata))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (crate::config::dangling_target(path), None)
            }
            Err(error) => return Err(error),
        };
        let mut options = std::fs::OpenOptions::new();
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
        let prepare = |file: &std::fs::File| {
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
        match crate::config::replace_file(&target, contents.as_bytes(), options, prepare) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                std::fs::write(&target, contents)
            }
            result => result,
        }
    })
    .await
    .map_err(std::io::Error::other)?
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
mod tests {
    use super::{MAX_OUTPUT, call};
    use serde_json::json;

    #[tokio::test]
    async fn rejects_invalid_numbers() {
        let cases = [
            (
                "bash",
                json!({ "command": "echo hi", "timeout": 0 }),
                "timeout must be at least 1",
            ),
            (
                "bash",
                json!({ "command": "echo hi", "timeout": -1 }),
                "timeout must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "limit": 0 }),
                "limit must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "limit": -1 }),
                "limit must be at least 1",
            ),
            (
                "read",
                json!({ "path": "Cargo.toml", "offset": -5 }),
                "offset must be at least 1",
            ),
        ];
        for (name, input, error) in cases {
            assert_eq!(call(name, &input).await, Err(error.into()), "{input}");
        }
    }

    #[tokio::test]
    async fn refuses_to_replace_a_file_that_changed_since_it_was_read() {
        let file = super::test_support::TemporaryFile::new("changed", "changed by someone else");
        let path = file.path().to_str().unwrap();
        let result = super::replace_file(path, "edited".into(), Some("as read".into())).await;
        assert_eq!(
            result.map_err(|error| error.to_string()),
            Err("it changed while being edited".into())
        );
        assert_eq!(file.content(), "changed by someone else");
    }

    #[test]
    fn truncates_inside_multibyte_character() {
        let text = super::truncate(format!("a{}", "é".repeat(MAX_OUTPUT)));
        assert!(text.ends_with("\n… output truncated"));
        assert_eq!(text.len(), MAX_OUTPUT - 1 + "\n… output truncated".len());
    }
}
