use std::{
    fs::OpenOptions,
    io::Write,
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
    if let Some(path) = CONFIG_DIR.get() {
        return Some(path.clone());
    }
    default_dir()
}

#[cfg(not(test))]
fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rust-claude"))
}

#[cfg(test)]
fn default_dir() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("rust-claude-test-config-{}", std::process::id())))
}

/// Creates a folder, and any missing parents, that only the user can open.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
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

/// Writes a file only the user can read and write, through a temporary file
/// renamed over it, so readers never see a half-written file and a failed
/// write keeps the old one.
pub fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicUsize = AtomicUsize::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = path.with_file_name(name);
    let written = private_file()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .and_then(|mut file| {
            // A file a crash left under this name may let others read it.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(contents)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Returns who may read, write and open the file or folder, as in `chmod`.
#[cfg(all(test, unix))]
pub(crate) fn permissions(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o777)
}
