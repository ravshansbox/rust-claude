use crate::tools::{call, test_support::TemporaryDir};
use serde_json::{Value, json};

const SOURCE: &str =
    "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    add(1, 2);\n}\n";

fn project(name: &str) -> TemporaryDir {
    let directory = TemporaryDir::new(name);
    std::fs::write(directory.path().join("a.rs"), SOURCE).unwrap();
    std::fs::write(directory.path().join("b.txt"), "Add numbers\n").unwrap();
    directory
}

async fn grep(directory: &TemporaryDir, input: Value) -> Result<String, String> {
    let mut input = input;
    input["path"] = json!(directory.path());
    let output = call("grep", &input).await;
    let prefix = format!("{}/", directory.path().display());
    output.map(|text| text.replace(&prefix, ""))
}

#[tokio::test]
async fn finds_matching_lines_in_each_file() {
    let directory = project("grep-match");
    assert_eq!(
        grep(&directory, json!({ "pattern": "add\\(" })).await,
        Ok("a.rs:1:fn add(a: i32, b: i32) -> i32 {\na.rs:6:    add(1, 2);\n".into())
    );
}

#[tokio::test]
async fn ignores_case_and_filters_files_by_glob() {
    let directory = project("grep-options");
    assert_eq!(
        grep(
            &directory,
            json!({ "pattern": "add", "ignore_case": true, "glob": "*.txt" })
        )
        .await,
        Ok("b.txt:1:Add numbers\n".into())
    );
}

#[tokio::test]
async fn searches_for_literal_text() {
    let directory = project("grep-literal");
    assert_eq!(
        grep(&directory, json!({ "pattern": "a + b", "literal": true })).await,
        Ok("a.rs:2:    a + b\n".into())
    );
}

#[tokio::test]
async fn shows_lines_around_matches() {
    let directory = project("grep-context");
    assert_eq!(
        grep(&directory, json!({ "pattern": "a \\+ b|add\\(1", "context": 1 })).await,
        Ok("a.rs-1-fn add(a: i32, b: i32) -> i32 {\na.rs:2:    a + b\na.rs-3-}\n--\na.rs-5-fn main() {\na.rs:6:    add(1, 2);\na.rs-7-}\n".into())
    );
}

#[tokio::test]
async fn stops_at_the_match_limit() {
    let directory = project("grep-limit");
    assert_eq!(
        grep(&directory, json!({ "pattern": "add", "limit": 1 })).await,
        Ok("a.rs:1:fn add(a: i32, b: i32) -> i32 {\n… stopped at 1 match, use limit for more or refine the pattern".into())
    );
}

#[tokio::test]
async fn says_when_nothing_matches() {
    let directory = project("grep-no-match");
    assert_eq!(
        grep(&directory, json!({ "pattern": "subtract" })).await,
        Ok("no matches".into())
    );
}

#[tokio::test]
async fn returns_ripgrep_errors() {
    let directory = project("grep-error");
    let error = grep(&directory, json!({ "pattern": "add(" }))
        .await
        .unwrap_err();
    assert!(error.contains("regex parse error"), "{error}");
}

#[tokio::test]
async fn checks_its_arguments() {
    let cases = [
        (json!({}), "missing argument: pattern"),
        (
            json!({ "pattern": "x", "limit": 0 }),
            "limit must be at least 1",
        ),
        (
            json!({ "pattern": "x", "context": -1 }),
            "context must be at least 0",
        ),
    ];
    for (input, error) in cases {
        assert_eq!(call("grep", &input).await, Err(error.into()), "{input}");
    }
}
