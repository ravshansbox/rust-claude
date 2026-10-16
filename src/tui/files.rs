use super::App;
use ignore::WalkBuilder;
use std::{path::Path, process::Command};

mod folders;

#[cfg(test)]
pub(super) use folders::folder_path;
pub(super) use folders::{FolderList, list_folder, path_query};

const MAX_MATCHES: usize = 10;
const MAX_FILES: usize = 10_000;

pub(super) fn list_files() -> Vec<String> {
    files_in(Path::new("."))
}

fn files_in(directory: &Path) -> Vec<String> {
    git_files(directory).unwrap_or_else(|| walk(directory, MAX_FILES))
}

fn git_files(directory: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(directory)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty() && directory.join(path).is_file())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

fn walk(root: &Path, limit: usize) -> Vec<String> {
    let mut files: Vec<String> = WalkBuilder::new(root)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != "target")
        .sort_by_file_name(Ord::cmp)
        .build()
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
        })
        .filter_map(|entry| {
            let path = entry.path().strip_prefix(root).ok()?;
            Some(path.to_string_lossy().into_owned())
        })
        .take(limit)
        .collect();
    files.sort();
    files
}

pub(super) fn file_query(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = &input[..cursor];
    let word = before.rsplit(char::is_whitespace).next()?;
    word.strip_prefix('@')
        .map(|query| (cursor - word.len(), query))
}

/// The repository paths offered after `@`, with their lowercase forms worked
/// out once, as matching runs on every key press and redraw.
pub(super) struct FileList {
    paths: Vec<String>,
    lowercase: Vec<String>,
}

impl FileList {
    pub(super) fn new(paths: Vec<String>) -> Self {
        let lowercase = paths.iter().map(|path| path.to_lowercase()).collect();
        Self { paths, lowercase }
    }

    /// Paths containing the query, ignoring case: the path equal to it
    /// first, then paths starting with it, then the rest, so Enter picks
    /// the file the user typed out in full.
    pub(super) fn matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        let mut exact = Vec::new();
        let mut prefix = Vec::new();
        let mut rest = Vec::new();
        for (path, lowercase) in self.paths.iter().zip(&self.lowercase) {
            if *lowercase == query {
                exact.push(path);
            } else if lowercase.starts_with(&query) {
                if prefix.len() < MAX_MATCHES {
                    prefix.push(path);
                }
            } else if lowercase.contains(&query) && rest.len() < MAX_MATCHES {
                rest.push(path);
            }
        }
        exact
            .into_iter()
            .chain(prefix)
            .chain(rest)
            .take(MAX_MATCHES)
            .cloned()
            .collect()
    }
}

impl App {
    /// Whether the file list should start loading now, for an `@` the user
    /// just typed. Returns the list's generation once until the list arrives.
    pub(super) fn start_listing_files(&mut self) -> Option<u64> {
        if self.files.is_some()
            || self.listing_files
            || file_query(&self.input, self.cursor).is_none()
        {
            return None;
        }
        self.listing_files = true;
        Some(self.files_generation)
    }

    pub(super) fn set_files(&mut self, generation: u64, files: Vec<String>) {
        if generation == self.files_generation {
            self.files = Some(FileList::new(files));
            self.listing_files = false;
        }
    }

    /// Makes the next `@` read the files again, as the prompt may change them.
    pub(super) fn forget_files(&mut self) {
        self.files = None;
        self.listing_files = false;
        self.folder = None;
        self.listing_folder = None;
        self.files_generation += 1;
    }
}

#[cfg(test)]
mod tests;
