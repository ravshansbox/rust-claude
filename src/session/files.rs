use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::Result;

use super::{EXTENSION, read::read_folder};

pub(super) fn latest_in_folder(directory: &Path, folder: &str) -> Result<Option<String>> {
    let mut files = session_files(directory)?;
    files.sort_by_key(|file| Reverse(file.modified));
    Ok(files
        .into_iter()
        .find(|file| read_folder(&file.path).ok().flatten().as_deref() == Some(folder))
        .map(|file| file.id))
}

pub(super) struct SessionFile {
    pub(super) id: String,
    pub(super) path: PathBuf,
    pub(super) modified: SystemTime,
}

/// Lists the session files in `directory`, skipping those whose name or
/// change time can't be read.
pub(super) fn session_files(directory: &Path) -> Result<Vec<SessionFile>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let paths = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(paths
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == EXTENSION)
        })
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?.to_string();
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()?;
            Some(SessionFile { id, path, modified })
        })
        .collect())
}
