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
fn default_dir() -> Option<PathBuf> {
    static DIR: std::sync::LazyLock<(PathBuf, std::fs::File)> = std::sync::LazyLock::new(|| {
        test_dir(&std::env::temp_dir().join("rust-coding-agent-test-config"))
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
/// write keeps the old one. A symlink there stays, and its file is replaced.
pub fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            dangling_target(path.to_path_buf())
        }
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
    prepare: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
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
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
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
    use super::{WRITES, permissions, replace_file, write_private_file};
    use std::sync::atomic::Ordering;

    #[cfg(unix)]
    #[test]
    fn saves_through_symlinked_file() {
        let base = std::env::temp_dir().join(format!(
            "rust-coding-agent-linked-save-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let target = base.join("dotfiles-settings.json");
        let link = base.join("settings.json");
        std::fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let result = write_private_file(&link, b"{\"model\":\"opus\"}");
        let found = (
            std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            std::fs::read_to_string(&target).unwrap(),
            permissions(&target),
        );
        std::fs::remove_dir_all(&base).unwrap();
        result.unwrap();
        assert_eq!(found, (true, "{\"model\":\"opus\"}".into(), Some(0o600)));
    }

    /// A link to a file not made yet stays, and the file is made.
    #[cfg(unix)]
    #[test]
    fn saves_through_dangling_symlink() {
        let base = std::env::temp_dir().join(format!(
            "rust-coding-agent-dangling-save-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("dotfiles")).unwrap();
        let link = base.join("settings.json");
        std::os::unix::fs::symlink("dotfiles/settings.json", &link).unwrap();
        let result = write_private_file(&link, b"{}");
        let found = (
            std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            std::fs::read_to_string(base.join("dotfiles/settings.json")).ok(),
        );
        std::fs::remove_dir_all(&base).unwrap();
        result.unwrap();
        assert_eq!(found, (true, Some("{}".into())));
    }

    /// A file someone links to from the next temporary names is left alone.
    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks_at_temporary_names() {
        let base =
            std::env::temp_dir().join(format!("rust-coding-agent-planted-{}", std::process::id()));
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
        let result = replace_file(&path, b"new", std::fs::OpenOptions::new(), |_| Ok(()));
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
}
