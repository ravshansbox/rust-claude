use super::create_private_dir;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub(super) fn default_dir() -> Option<PathBuf> {
    static DIR: LazyLock<(PathBuf, File)> =
        LazyLock::new(|| test_dir(&std::env::temp_dir().join("rust-coding-agent-test-config")));
    Some(DIR.0.clone())
}

/// Gives this test process its own folder under `base`, locked while the
/// returned file stays open, and removes the folders of test processes that
/// have exited.
fn test_dir(base: &Path) -> (PathBuf, File) {
    let id = std::process::id().to_string();
    // Lock the folder before giving it the name other processes look at, so
    // they never see it unlocked.
    let unnamed = base.join(format!(".{id}"));
    let _ = std::fs::remove_dir_all(&unnamed);
    create_private_dir(&unnamed).unwrap();
    let lock = File::open(&unnamed).unwrap();
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
        if let Ok(folder) = File::open(&path)
            && folder.try_lock().is_ok()
        {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Returns who may read, write and open the file or folder, as in `chmod`.
#[cfg(unix)]
pub(crate) fn permissions(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o777)
}
