use serde_json::{Value, json};

mod bash;
mod display;
mod edit;
mod read;
#[cfg(test)]
mod test_support;
mod write;

pub use bash::{ProcessGroup, new_session};
pub use display::{ReadGroup, diff, note, summary};

const MAX_OUTPUT: usize = 20_000;

pub fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… output truncated");
    }
    text
}

pub fn definitions() -> Value {
    json!([
        bash::definition(),
        read::definition(),
        write::definition(),
        edit::definition()
    ])
}

fn argument<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("missing argument: {key}"))
}

/// Replaces the file at `path`, or the file a symlink there points to, in one
/// step, keeping the permissions of the file it replaces. In a folder where
/// no temporary file can be created, it writes the file in place instead.
async fn replace_file(path: &str, contents: String) -> std::io::Result<()> {
    let path = std::path::PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        let (target, permissions) = match std::fs::canonicalize(&path) {
            Ok(target) => {
                // Refuses a file the user made read-only, as writing in place would.
                let file = std::fs::OpenOptions::new().write(true).open(&target)?;
                let permissions = file.metadata()?.permissions();
                (target, Some(permissions))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (dangling_target(path), None)
            }
            Err(error) => return Err(error),
        };
        let mut options = std::fs::OpenOptions::new();
        #[cfg(unix)]
        if let Some(permissions) = &permissions {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(permissions.mode() & 0o777);
        }
        match crate::config::replace_file(&target, contents.as_bytes(), options, permissions) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                std::fs::write(&target, contents)
            }
            result => result,
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Follows symlinks at a path that does not exist yet to the file they would
/// create, so writing it keeps the links.
fn dangling_target(mut path: std::path::PathBuf) -> std::path::PathBuf {
    // Stops at a loop of links, as the system does.
    for _ in 0..40 {
        let Ok(link) = std::fs::read_link(&path) else {
            break;
        };
        path = match path.parent() {
            Some(parent) => parent.join(link),
            None => link,
        };
    }
    path
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

    #[test]
    fn truncates_inside_multibyte_character() {
        let text = super::truncate(format!("a{}", "é".repeat(MAX_OUTPUT)));
        assert!(text.ends_with("\n… output truncated"));
        assert_eq!(text.len(), MAX_OUTPUT - 1 + "\n… output truncated".len());
    }
}
