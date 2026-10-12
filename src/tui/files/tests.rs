use super::{file_query, files_in, walk};
use crate::tools::test_support::TemporaryDir;
use std::process::Command;

#[test]
fn lists_git_files_with_non_ascii_names() {
    let folder = TemporaryDir::new("files");
    let root = folder.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/café.rs"), "").unwrap();
    std::fs::write(root.join("plain.rs"), "").unwrap();
    let initialised = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success();
    let files = files_in(root);
    assert!(initialised);
    assert_eq!(files, ["plain.rs", "src/café.rs"]);
}

#[test]
fn skips_ignored_files_outside_git() {
    let folder = TemporaryDir::new("walk");
    let root = folder.path();
    for subfolder in ["target", ".hidden", "logs"] {
        std::fs::create_dir_all(root.join(subfolder)).unwrap();
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
    let files = files_in(root);
    assert_eq!(files, ["a.rs", "b.rs"]);
}

#[test]
fn stops_walking_at_the_file_limit() {
    let folder = TemporaryDir::new("limit");
    let root = folder.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    for file in ["a.rs", "b.rs", "src/c.rs", "src/d.rs"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let all = walk(root, 10);
    let capped = walk(root, 3);
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
