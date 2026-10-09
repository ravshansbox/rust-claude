use std::path::{Path, PathBuf};

pub(super) struct TemporaryFile(PathBuf);

impl TemporaryFile {
    pub(super) fn new(name: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rust-claude-{name}-{}", std::process::id()));
        std::fs::write(&path, content).unwrap();
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
