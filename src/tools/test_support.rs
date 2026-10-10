use std::path::{Path, PathBuf};

pub(super) struct TemporaryFile(PathBuf);

impl TemporaryFile {
    pub(super) fn new(name: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rust-claude-{name}-{}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        Self(path)
    }

    /// A named pipe, which only reaches its end once every writer closes it.
    pub(super) fn fifo(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rust-claude-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        Self(path)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }

    pub(super) fn content(&self) -> String {
        std::fs::read_to_string(&self.0).unwrap()
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A folder removed with everything in it when dropped.
pub(super) struct TemporaryDir(PathBuf);

impl TemporaryDir {
    pub(super) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rust-claude-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }

    /// Names of the entries in the folder, sorted.
    pub(super) fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Sets the folder's permission bits, as in `chmod`.
    #[cfg(unix)]
    pub(super) fn set_mode(&self, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}

impl Drop for TemporaryDir {
    fn drop(&mut self) {
        #[cfg(unix)]
        self.set_mode(0o700);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
