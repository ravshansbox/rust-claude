use super::{create_private_dir, private_file};
use std::{
    fs::{File, OpenOptions},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// Writes a file only the user can read and write, through a temporary file
/// renamed over it, so readers never see a half-written file and a failed
/// write keeps the old one. A symlink there stays, and its file is replaced.
pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == ErrorKind::NotFound => dangling_target(path.to_path_buf()),
        Err(error) => return Err(error),
    };
    let path = target.as_path();
    // A file a crash left under the temporary name may let others read it.
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::Permissions::from_mode(0o600))
    };
    #[cfg(not(unix))]
    let permissions = None;
    replace_file(path, contents, private_file(), |file| {
        permissions.map_or(Ok(()), |permissions| file.set_permissions(permissions))
    })
}

/// Follows symlinks at a path that does not exist yet to the file they would
/// create, so writing it keeps the links.
pub(crate) fn dangling_target(mut path: PathBuf) -> PathBuf {
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

/// Counts temporary files this process made, to give each a new name.
pub(super) static WRITES: AtomicUsize = AtomicUsize::new(0);

/// Writes a file through a temporary file in the same folder, created with
/// `options` and handed to `prepare` before it is written, then renamed over
/// it, so readers never see a half-written file and a failed write keeps the
/// old one.
pub fn replace_file(
    path: &Path,
    contents: &[u8],
    mut options: OpenOptions,
    prepare: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    options.write(true).create_new(true);
    // Only the start of the name, so the temporary name stays within the
    // longest name a folder allows.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let prefix = &name[..name.floor_char_boundary(64)];
    // A new name each time, so a file or link someone else put at the
    // name is never opened.
    let (temporary, mut file) = loop {
        let temporary = path.with_file_name(format!(
            "{prefix}.{}.{}.tmp",
            std::process::id(),
            WRITES.fetch_add(1, Ordering::Relaxed)
        ));
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    };
    let written = (|| {
        prepare(&file)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}
