use super::on_path;
use crate::tools::{call, test_support::TemporaryDir};
use serde_json::json;
use std::{ffi::OsString, os::unix::fs::PermissionsExt};

const SOURCE: &str =
    "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    add(1, 2);\n}\n";

fn project(name: &str) -> (TemporaryDir, String) {
    let directory = TemporaryDir::new(name);
    let path = directory.path().join("a.rs");
    std::fs::write(&path, SOURCE).unwrap();
    (directory, path.to_str().unwrap().to_string())
}

#[tokio::test]
async fn searches_code_by_syntax() {
    let (directory, file) = project("ast-grep-search");
    let input = json!({ "command": "search", "pattern": "add($A, $B)", "path": directory.path() });
    assert_eq!(
        call("ast_grep", &input).await,
        Ok(format!("{file}:6:    add(1, 2);\n"))
    );
}

#[tokio::test]
async fn says_when_nothing_matches() {
    let (_directory, file) = project("ast-grep-no-match");
    let input =
        json!({ "command": "search", "pattern": "subtract($A, $B)", "path": file, "lang": "rust" });
    assert_eq!(call("ast_grep", &input).await, Ok("no matches".into()));
}

#[tokio::test]
async fn outlines_a_file() {
    let (_directory, file) = project("ast-grep-outline");
    let input = json!({ "command": "outline", "path": file });
    assert_eq!(
        call("ast_grep", &input).await,
        Ok(format!(
            "{file}\n1: fn add(a: i32, b: i32) -> i32\n5: fn main()\n"
        ))
    );
}

#[tokio::test]
async fn returns_ast_grep_errors() {
    let input = json!({ "command": "search", "pattern": "x", "path": "missing.rs" });
    let error = call("ast_grep", &input).await.unwrap_err();
    assert!(
        error.contains("missing.rs: No such file or directory"),
        "{error}"
    );
}

#[tokio::test]
async fn checks_its_arguments() {
    let cases = [
        (json!({ "command": "search" }), "missing argument: pattern"),
        (json!({ "pattern": "x" }), "missing argument: command"),
        (
            json!({ "command": "rewrite", "pattern": "x" }),
            "command must be search or outline",
        ),
    ];
    for (input, error) in cases {
        assert_eq!(call("ast_grep", &input).await, Err(error.into()), "{input}");
    }
}

#[test]
fn finds_programs_on_the_path() {
    let empty = TemporaryDir::new("path-empty");
    let installed = TemporaryDir::new("path-installed");
    let program = installed.path().join("ast-grep");
    std::fs::write(&program, "").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path =
        |dirs: &[&TemporaryDir]| std::env::join_paths(dirs.iter().map(|dir| dir.path())).unwrap();
    assert!(on_path("ast-grep", Some(path(&[&empty, &installed]))));
    assert!(!on_path("ast-grep", Some(path(&[&empty]))));
    assert!(!on_path("ast-grep", None::<OsString>));
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!on_path("ast-grep", Some(path(&[&installed]))));
}
