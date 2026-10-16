use super::{super::App, MAX_MATCHES, file_query};
use crate::config;
use std::path::PathBuf;

const PATH_STARTS: [&str; 4] = ["~/", "/", "./", "../"];

/// Splits a query such as `~/src/ma` into the folder `~/src/` and the start
/// of a name `ma`, when the query is a path rather than a project file.
pub(in crate::tui) fn path_query(query: &str) -> Option<(&str, &str)> {
    if !PATH_STARTS.iter().any(|start| query.starts_with(start)) {
        return None;
    }
    let split = query.rfind('/')? + 1;
    Some((&query[..split], &query[split..]))
}

pub(in crate::tui) fn folder_path(folder: &str, home: Option<PathBuf>) -> PathBuf {
    match (folder.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(folder),
    }
}

/// The names in a folder, sorted, with `/` after each folder.
pub(in crate::tui) fn list_folder(folder: &str) -> Vec<String> {
    let path = folder_path(folder, config::home());
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    names
}

pub(in crate::tui) struct FolderList {
    folder: String,
    names: Vec<String>,
}

impl FolderList {
    /// Paths in the folder whose names start with `name`, ignoring case.
    /// Hidden names show only when `name` starts with a dot, as in shells.
    pub(in crate::tui) fn matches(&self, folder: &str, name: &str) -> Option<Vec<String>> {
        if self.folder != folder {
            return None;
        }
        let name = name.to_lowercase();
        let hidden = name.starts_with('.');
        Some(
            self.names
                .iter()
                .filter(|entry| hidden || !entry.starts_with('.'))
                .filter(|entry| entry.to_lowercase().starts_with(&name))
                .take(MAX_MATCHES)
                .map(|entry| format!("{folder}{entry}"))
                .collect(),
        )
    }
}

impl App {
    /// The folder to list now, for a path the user typed after `@`, with
    /// the list's generation. Returns each folder once until its list arrives.
    pub(in crate::tui) fn start_listing_folder(&mut self) -> Option<(u64, String)> {
        let (_, query) = file_query(&self.input, self.cursor)?;
        let (folder, _) = path_query(query)?;
        let listed = self.folder.as_ref().map(|list| list.folder.as_str());
        if listed == Some(folder) || self.listing_folder.as_deref() == Some(folder) {
            return None;
        }
        self.listing_folder = Some(folder.to_string());
        Some((self.files_generation, folder.to_string()))
    }

    pub(in crate::tui) fn set_folder(
        &mut self,
        generation: u64,
        folder: String,
        names: Vec<String>,
    ) {
        if generation == self.files_generation && self.listing_folder.as_ref() == Some(&folder) {
            self.folder = Some(FolderList { folder, names });
            self.listing_folder = None;
        }
    }
}
