use std::{
    fs::OpenOptions,
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(path: PathBuf) {
    let _ = CONFIG_DIR.set(path);
}

pub fn dir() -> Option<PathBuf> {
    CONFIG_DIR.get().cloned().or_else(default_dir)
}

#[cfg(not(test))]
fn default_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let dir = home.join(".rust-coding-agent");
    let old = home.join(".rust-claude");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    Some(dir)
}

#[cfg(test)]
use test_support::default_dir;

/// Creates a folder, and any missing parents, that only the user can open.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// Returns options that create a file only the user can read and write.
pub fn private_file() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// Locks the file at `path`, creating it, until the returned file is closed.
/// Waits without a blocking lock, so quitting does not wait for another
/// copy of rust-coding-agent to let go.
pub async fn lock_file(path: &Path) -> io::Result<std::fs::File> {
    let file = private_file().create(true).write(true).open(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// Writes a file only the user can read and write, through a temporary file
/// renamed over it, so readers never see a half-written file and a failed
/// write keeps the old one. A symlink there stays, and its file is replaced.
pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
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
    replace_file(path, contents, private_file(), |file| match permissions {
        Some(permissions) => file.set_permissions(permissions),
        None => Ok(()),
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
static WRITES: AtomicUsize = AtomicUsize::new(0);

/// Writes a file through a temporary file in the same folder, created with
/// `options` and handed to `prepare` before it is written, then renamed over
/// it, so readers never see a half-written file and a failed write keeps the
/// old one.
pub fn replace_file(
    path: &Path,
    contents: &[u8],
    mut options: OpenOptions,
    prepare: impl FnOnce(&std::fs::File) -> io::Result<()>,
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

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
