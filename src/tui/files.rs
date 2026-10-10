use ignore::WalkBuilder;
use std::{path::Path, process::Command};

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
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

fn walk(root: &Path, limit: usize) -> Vec<String> {
    let mut files: Vec<String> = WalkBuilder::new(root)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != "target")
        .sort_by_file_name(|a, b| a.cmp(b))
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
    let start = before
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    before[start..]
        .strip_prefix('@')
        .map(|query| (start, query))
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

    pub(super) fn matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.paths
            .iter()
            .zip(&self.lowercase)
            .filter(|(_, lowercase)| lowercase.contains(&query))
            .take(MAX_MATCHES)
            .map(|(path, _)| path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileList, file_query, files_in, walk};
    use std::process::Command;

    #[test]
    fn lists_git_files_with_non_ascii_names() {
        let root = std::env::temp_dir().join(format!("rust-claude-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/café.rs"), "").unwrap();
        std::fs::write(root.join("plain.rs"), "").unwrap();
        let initialised = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success();
        let files = files_in(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert!(initialised);
        assert_eq!(files, ["plain.rs", "src/café.rs"]);
    }

    #[test]
    fn skips_ignored_files_outside_git() {
        let root = std::env::temp_dir().join(format!("rust-claude-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for folder in ["target", ".hidden", "logs"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        for file in [
            "a.rs",
            "b.rs",
            "skip.log",
            "logs/out.txt",
            "target/out.rs",
            ".hidden/x.rs",
        ] {
            std::fs::write(root.join(file), "").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(root.join(".ignore"), "logs/\n").unwrap();
        let files = files_in(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(files, ["a.rs", "b.rs"]);
    }

    #[test]
    fn stops_walking_at_the_file_limit() {
        let root = std::env::temp_dir().join(format!("rust-claude-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        for file in ["a.rs", "b.rs", "src/c.rs", "src/d.rs"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let all = walk(&root, 10);
        let capped = walk(&root, 3);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(all, ["a.rs", "b.rs", "src/c.rs", "src/d.rs"]);
        assert_eq!(capped.len(), 3);
    }

    #[test]
    fn finds_file_query_at_cursor() {
        assert_eq!(file_query("read @src/ma", 12), Some((5, "src/ma")));
        assert_eq!(file_query("@", 1), Some((0, "")));
        assert_eq!(file_query("mail@host", 9), None);
        assert_eq!(file_query("@src now", 8), None);
    }

    #[test]
    fn matches_files_ignoring_case() {
        let files = FileList::new(vec!["README.md".to_string(), "src/main.rs".to_string()]);
        assert_eq!(files.matches("readme"), vec!["README.md"]);
        assert_eq!(files.matches("").len(), 2);
    }
}
