use std::path::{Path, PathBuf};

/// Whether the process is alive. A zombie, killed but not yet reaped,
/// counts as stopped.
pub(crate) fn is_running(pid: &str) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid.trim()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    !state.trim().is_empty() && !state.trim_start().starts_with('Z')
}

fn temporary_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rust-coding-agent-{name}-{}", std::process::id()))
}

pub(crate) struct TemporaryFile(PathBuf);

impl TemporaryFile {
    pub(crate) fn new(name: &str, content: &str) -> Self {
        let path = temporary_path(name);
        std::fs::write(&path, content).unwrap();
        Self(path)
    }

    /// A named pipe, which only reaches its end once every writer closes it.
    pub(super) fn fifo(name: &str) -> Self {
        let path = temporary_path(name);
        let _ = std::fs::remove_file(&path);
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn content(&self) -> String {
        std::fs::read_to_string(&self.0).unwrap()
    }

    /// Waits up to five seconds for the file to hold more than white space,
    /// and gives what it holds.
    pub(crate) async fn wait_for_content(&self) -> String {
        let mut content = String::new();
        for _ in 0..100 {
            content = self.content();
            if !content.trim().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        content
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A folder removed with everything in it when dropped.
pub(crate) struct TemporaryDir(PathBuf);

impl TemporaryDir {
    pub(crate) fn new(name: &str) -> Self {
        let path = temporary_path(name);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
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
