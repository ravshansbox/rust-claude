use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, ErrorKind},
};

use crate::config;

pub(super) fn failed(action: &str, path: &str) -> impl Fn(io::Error) -> String + Copy {
    move |error| format!("failed to {action} {path}: {error}")
}

pub(super) async fn regular_file(path: &str) -> Result<Metadata, String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(failed("read", path))?;
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
pub(super) async fn replace_file(
    path: &str,
    contents: String,
    expected: Option<String>,
) -> io::Result<()> {
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
