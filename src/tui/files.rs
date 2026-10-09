use std::{path::Path, process::Command};

const MAX_MATCHES: usize = 10;

pub(super) fn list_files() -> Vec<String> {
    git_files().unwrap_or_else(|| {
        let mut files = Vec::new();
        walk(Path::new("."), &mut files);
        files.sort();
        files
    })
}

fn git_files() -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|path| Path::new(path).is_file())
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

fn walk(directory: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(&path, files);
        } else if file_type.is_file() {
            let path = path.strip_prefix(".").unwrap_or(&path);
            files.push(path.to_string_lossy().into_owned());
        }
    }
}

pub(super) fn file_query(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = &input[..cursor];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    before[start..]
        .strip_prefix('@')
        .map(|query| (start, query))
}

pub(super) fn file_matches(files: &[String], query: &str) -> Vec<String> {
    let query = query.to_lowercase();
    files
        .iter()
        .filter(|path| path.to_lowercase().contains(&query))
        .take(MAX_MATCHES)
        .cloned()
        .collect()
}
