use std::{
    fs::{DirBuilder, File, OpenOptions, TryLockError},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

mod write;

#[cfg(test)]
use write::WRITES;
pub(crate) use write::dangling_target;
pub use write::{replace_file, write_private_file};

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(path: PathBuf) {
    let _ = CONFIG_DIR.set(path);
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn dir() -> Option<PathBuf> {
    CONFIG_DIR.get().cloned().or_else(default_dir)
}

pub fn file(name: &str) -> anyhow::Result<PathBuf> {
    dir()
        .map(|dir| dir.join(name))
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))
}

#[cfg(not(test))]
fn default_dir() -> Option<PathBuf> {
    let home = home()?;
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
    private_dir().recursive(true).create(path)
}

/// Returns a builder that creates a folder only the user can open.
pub fn private_dir() -> DirBuilder {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
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
pub async fn lock_file(path: &Path) -> io::Result<File> {
    let file = open_lock_file(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

pub fn lock_file_blocking(path: &Path) -> io::Result<File> {
    let file = open_lock_file(path)?;
    file.lock()?;
    Ok(file)
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    private_file().create(true).write(true).open(path)
}

/// Appends `contents` to `file`, which is `length` long, and cuts off a
/// partly written line on failure, so the next write starts on a new line.
pub fn append_whole(file: &mut File, length: u64, contents: &[u8]) -> io::Result<()> {
    let written = file.write_all(contents);
    if written.is_err() {
        let _ = file.set_len(length);
    }
    written
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
