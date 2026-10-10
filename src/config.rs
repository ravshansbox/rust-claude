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
    static DIR: std::sync::LazyLock<(PathBuf, std::fs::File)> = std::sync::LazyLock::new(|| {
        test_dir(&std::env::temp_dir().join("rust-claude-test-config"))
    });
    Some(DIR.0.clone())
}

/// Gives this test process its own folder under `base`, locked while the
/// returned file stays open, and removes the folders of test processes that
/// have exited.
#[cfg(test)]
fn test_dir(base: &Path) -> (PathBuf, std::fs::File) {
    let id = std::process::id().to_string();
    // Lock the folder before giving it the name other processes look at, so
    // they never see it unlocked.
    let unnamed = base.join(format!(".{id}"));
    let _ = std::fs::remove_dir_all(&unnamed);
    create_private_dir(&unnamed).unwrap();
    let lock = std::fs::File::open(&unnamed).unwrap();
    lock.lock().unwrap();
    let own = base.join(&id);
    // A folder with this id is left from an exited process, and another
    // process may be removing it too.
    let mut attempts = 0;
    while let Err(error) = std::fs::rename(&unnamed, &own) {
        attempts += 1;
        assert!(attempts < 100, "{error}");
        let _ = std::fs::remove_dir_all(&own);
    }
    remove_unlocked(base, &id);
    (own, lock)
}

/// Removes the test folders in `base`, other than `own`, that no running
/// process holds a lock on.
#[cfg(test)]
fn remove_unlocked(base: &Path, own: &str) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == own || name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if let Ok(folder) = std::fs::File::open(&path)
            && folder.try_lock().is_ok()
        {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
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
    // A file a crash left under the temporary name may let others read it.
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::Permissions::from_mode(0o600))
    };
    #[cfg(not(unix))]
    let permissions = None;
    replace_file(path, contents, private_file(), permissions)
}

/// Counts temporary files this process made, to give each a new name.
static WRITES: AtomicUsize = AtomicUsize::new(0);

/// Writes a file through a temporary file in the same folder, created with
/// `options` and given `permissions`, then renamed over it, so readers never
/// see a half-written file and a failed write keeps the old one.
pub fn replace_file(
    path: &Path,
    contents: &[u8],
    mut options: OpenOptions,
    permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    options.write(true).create_new(true);
    // A new name each time, so a file or link someone else put at the
    // name is never opened.
    let (temporary, mut file) = loop {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(
            ".{}.{}.tmp",
            std::process::id(),
            WRITES.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = path.with_file_name(name);
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let written = (|| {
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
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

#[cfg(test)]
mod tests {
    use super::{WRITES, replace_file, test_dir};
    use std::sync::atomic::Ordering;

    /// A file someone links to from the next temporary names is left alone.
    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks_at_temporary_names() {
        let base = std::env::temp_dir().join(format!("rust-claude-planted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let victim = base.join("victim");
        std::fs::write(&victim, "secret").unwrap();
        let path = base.join("file.log");
        let next = WRITES.load(Ordering::Relaxed);
        for counter in next..next + 64 {
            let planted = base.join(format!("file.log.{}.{counter}.tmp", std::process::id()));
            std::os::unix::fs::symlink(&victim, planted).unwrap();
        }
        let result = replace_file(&path, b"new", std::fs::OpenOptions::new(), None);
        let found = (
            std::fs::read_to_string(&victim).unwrap(),
            std::fs::read_to_string(&path).ok(),
            std::fs::symlink_metadata(&path)
                .map(|metadata| metadata.is_symlink())
                .ok(),
        );
        std::fs::remove_dir_all(&base).unwrap();
        result.unwrap();
        assert_eq!(found, ("secret".into(), Some("new".into()), Some(false)));
    }

    #[test]
    fn removes_test_folders_of_exited_processes() {
        let base =
            std::env::temp_dir().join(format!("rust-claude-test-folders-{}", std::process::id()));
        let exited = base.join("1");
        let running = base.join("2");
        let reused = base.join(std::process::id().to_string());
        for folder in [&exited, &running, &reused] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("settings.json"), "{}").unwrap();
        }
        let running_lock = std::fs::File::open(&running).unwrap();
        running_lock.lock().unwrap();
        let (own, _lock) = test_dir(&base);
        let own_is_locked = std::fs::File::open(&own).unwrap().try_lock().is_err();
        let found = (
            exited.exists(),
            running.exists(),
            own == reused,
            std::fs::read_dir(&own).unwrap().count(),
            own_is_locked,
        );
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(found, (false, true, true, 0, true));
    }
}
