use super::{diff, note, summary};
use serde_json::json;

#[test]
fn previews_first_lines_of_write() {
    let content: String = (1..=12).map(|number| format!("line {number}\n")).collect();
    let preview = diff("write", &json!({ "path": "a.txt", "content": content })).unwrap();
    let lines: Vec<&str> = preview.lines().collect();
    assert_eq!(lines.len(), 11);
    assert_eq!(lines[0], " line 1");
    assert_eq!(lines[9], " line 10");
    assert_eq!(lines[10], "… 2 more lines");
}

#[test]
fn shows_read_range_in_summary() {
    let cases = [
        (json!({ "path": "a.rs" }), "a.rs"),
        (
            json!({ "path": "a.rs", "offset": 325, "limit": 30 }),
            "a.rs:325-354",
        ),
        (json!({ "path": "a.rs", "offset": 325 }), "a.rs:325-"),
        (json!({ "path": "a.rs", "limit": 30 }), "a.rs:1-30"),
        (
            json!({ "path": "a.rs", "offset": 5, "limit": u64::MAX }),
            &format!("a.rs:5-{}", u64::MAX),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(summary("read", &input), expected);
    }
}

#[test]
fn shows_ast_grep_command_pattern_and_path_in_summary() {
    let cases = [
        (
            json!({ "command": "search", "pattern": "add($A)" }),
            "search add($A)",
        ),
        (
            json!({ "command": "search", "pattern": "add($A)", "path": "src" }),
            "search add($A) in src",
        ),
        (
            json!({ "command": "outline", "path": "src/a.rs" }),
            "outline src/a.rs",
        ),
        (json!({ "command": "outline" }), "outline"),
    ];
    for (input, expected) in cases {
        assert_eq!(summary("ast_grep", &input), expected);
    }
}

#[test]
fn shows_grep_pattern_and_path_in_summary() {
    assert_eq!(summary("grep", &json!({ "pattern": "fn main" })), "fn main");
    assert_eq!(
        summary("grep", &json!({ "pattern": "fn main", "path": "src" })),
        "fn main in src"
    );
}

#[test]
fn notes_replacement_count() {
    assert_eq!(
        note("edit", "edited a (b).rs (3 replacements)"),
        Some("3 replacements".into())
    );
    assert_eq!(
        note("edit", "edited a.rs (1 replacement)"),
        Some("1 replacement".into())
    );
    assert_eq!(note("edit", "edited a (b).rs"), None);
    assert_eq!(note("bash", "x (3 replacements)"), None);
}

#[test]
fn diffs_edit_input() {
    let input = json!({ "path": "a", "old_text": "a\nb\n", "new_text": "a\nc\n" });
    assert_eq!(diff("edit", &input), Some(" a\n-b\n+c".into()));
    assert_eq!(diff("write", &input), None);
}

#[test]
fn keeps_last_line_unchanged_when_lines_are_appended_without_newline() {
    let input = json!({ "path": "a", "old_text": "foo()", "new_text": "foo()\nbar()" });
    assert_eq!(diff("edit", &input), Some(" foo()\n+bar()".into()));
}

#[test]
fn shows_bash_command_as_code() {
    let input = json!({ "command": "for x in a b; do\n    echo $x\ndone" });
    assert_eq!(summary("bash", &input), "");
    assert_eq!(
        diff("bash", &input),
        Some("for x in a b; do\n echo $x\ndone".into())
    );
}

#[test]
fn keeps_one_space_per_indentation_level_in_previews() {
    let edit = json!({ "path": "a", "old_text": "a\n    b\n", "new_text": "a\n        c\n" });
    assert_eq!(diff("edit", &edit), Some(" a\n- b\n+  c".into()));
    let tabs = json!({ "path": "a", "old_text": "a\n\tb\n", "new_text": "a\n\t\tc\n" });
    assert_eq!(diff("edit", &tabs), Some(" a\n- b\n+  c".into()));
    let write = json!({ "path": "a", "content": "fn main() {\n  if x {\n    body\n  }\n}\n" });
    assert_eq!(
        diff("write", &write),
        Some(" fn main() {\n  if x {\n   body\n  }\n }".into())
    );
}
