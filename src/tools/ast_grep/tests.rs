use crate::tools::{call, test_support::TemporaryDir};
use serde_json::json;

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

#[tokio::test]
async fn searches_for_patterns_that_start_with_a_dash() {
    let (_directory, file) = project("ast-grep-dash");
    std::fs::write(&file, "fn main() {\n    let x = -1;\n}\n").unwrap();
    let input = json!({ "command": "search", "pattern": "-1", "path": file, "lang": "rust" });
    assert_eq!(
        call("ast_grep", &input).await,
        Ok(format!("{file}:2:    let x = -1;\n"))
    );
}

#[tokio::test]
async fn truncates_long_output() {
    let (directory, file) = project("ast-grep-long");
    std::fs::write(&file, "fn main() {\n    add(1, 2);\n}\n".repeat(2_000)).unwrap();
    let input = json!({ "command": "search", "pattern": "add($A, $B)", "path": directory.path() });
    let output = call("ast_grep", &input).await.unwrap();
    assert!(output.ends_with("\n… output truncated"), "{output}");
}
