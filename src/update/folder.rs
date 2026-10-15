use std::{
    io,
    path::{Path, PathBuf},
};

use crate::config;

pub(super) fn create_update_folder(folder: &Path) -> io::Result<()> {
    config::private_dir().create(folder)
}

/// An update's own folder, deleted when the update ends, even when the app
/// quits halfway and drops it.
pub(super) struct UpdateFolder(pub(super) PathBuf);

impl Drop for UpdateFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
