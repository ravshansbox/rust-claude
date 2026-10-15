use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::SKILL_FILE;

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    RustClaude,
    Agents,
}

pub(super) fn collect_skill_files(dir: &Path, mode: Mode) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_from(
        dir,
        dir,
        mode,
        &mut Vec::new(),
        &mut HashSet::new(),
        &mut files,
    );
    files
}

fn ignore_rules(dir: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);
    for file_name in IGNORE_FILE_NAMES {
        let path = dir.join(file_name);
        if path.is_file() {
            let _ = builder.add(path);
        }
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// Asks the deepest directory's ignore files first, like git does.
fn is_ignored(rules: &[Gitignore], path: &Path, is_dir: bool) -> bool {
    rules
        .iter()
        .rev()
        .map(|rules| rules.matched(path, is_dir))
        .find(|matched| !matched.is_none())
        .is_some_and(|matched| matched.is_ignore())
}

fn collect_from(
    dir: &Path,
    root: &Path,
    mode: Mode,
    rules: &mut Vec<Gitignore>,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let Ok(real_dir) = dir.canonicalize() else {
        return;
    };
    if !visited.insert(real_dir) {
        return;
    }
    rules.push(ignore_rules(dir));
    let skill_file = dir.join(SKILL_FILE);
    if skill_file.is_file() && !is_ignored(rules, &skill_file, false) {
        files.push(skill_file);
        rules.pop();
        return;
    }

    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let include_markdown = metadata.is_file()
            && name.ends_with(".md")
            && !is_ignored(rules, &path, false)
            && match mode {
                Mode::RustClaude => dir == root,
                Mode::Agents => dir != root,
            };
        if include_markdown {
            files.push(path);
            continue;
        }
        if !metadata.is_dir() || is_ignored(rules, &path, true) {
            continue;
        }
        collect_from(&path, root, mode, rules, visited, files);
    }
    rules.pop();
}
