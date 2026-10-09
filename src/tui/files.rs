use std::{path::Path, process::Command};

const MAX_MATCHES: usize = 10;

pub(super) fn list_files() -> Vec<String> {
    files_in(Path::new("."))
}

fn files_in(directory: &Path) -> Vec<String> {
    git_files(directory).unwrap_or_else(|| {
        let mut files = Vec::new();
        walk(directory, directory, &mut files);
        files.sort();
        files
    })
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

fn walk(root: &Path, directory: &Path, files: &mut Vec<String>) {
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
            walk(root, &path, files);
        } else if file_type.is_file() {
            let path = path.strip_prefix(root).unwrap_or(&path);
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

#[cfg(test)]
mod tests {
    use super::{file_matches, file_query, files_in};
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
    fn finds_file_query_at_cursor() {
        assert_eq!(file_query("read @src/ma", 12), Some((5, "src/ma")));
        assert_eq!(file_query("@", 1), Some((0, "")));
        assert_eq!(file_query("mail@host", 9), None);
        assert_eq!(file_query("@src now", 8), None);
    }

    #[test]
    fn matches_files_ignoring_case() {
        let files = vec!["README.md".to_string(), "src/main.rs".to_string()];
        assert_eq!(file_matches(&files, "readme"), vec!["README.md"]);
        assert_eq!(file_matches(&files, "").len(), 2);
    }
}
