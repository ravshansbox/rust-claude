use super::*;

#[test]
fn keeps_quoted_code_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "> ```\n> let x = 1;\n> ```\n\n> ```\n> y\nfirst\nsecond",
        40,
    ));
    let code = lines.iter().find(|line| line.contains("let x")).unwrap();
    assert!(code.ends_with("let x = 1;"), "{lines:?}");
    assert_eq!(lines[lines.len() - 2..], ["first", "second"]);
}

#[test]
fn keeps_indented_code_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "Run:\n\n    let x = 1;\n    let y = 2;\n\n- item\n\n      z\n\n1. step\n\n    first\n    second",
        40,
    ));
    let code: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("let ") || line.trim_end() == "z")
        .collect();
    assert_eq!(code, ["let x = 1;", "let y = 2;", "z"], "{lines:?}");
    assert_eq!(lines[lines.len() - 2..], ["first", "second"]);
}

#[test]
fn keeps_indented_code_after_a_heading_or_fence_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "## Setup\n    let x = 1;\n```\nfenced\n```\n    let y = 2;",
        40,
    ));
    let code: Vec<_> = lines.iter().filter(|line| line.contains("let ")).collect();
    assert_eq!(code, ["let x = 1;", "let y = 2;"], "{lines:?}");
}

#[test]
fn keeps_indented_code_after_a_rule_or_underline_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "Intro\n\n***\n    let x = 1;\n\nTitle\n=====\n    let y = 2;\n\n* * *\n    let z = 3;",
        40,
    ));
    let code: Vec<_> = lines.iter().filter(|line| line.contains("let ")).collect();
    assert_eq!(
        code,
        ["let x = 1;", "let y = 2;", "let z = 3;"],
        "{lines:?}"
    );
}

#[test]
fn keeps_code_quoted_in_a_list_item_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "- > ```\n  > let x = 1;\n  > ```\n  > first\n  > second",
        40,
    ));
    let code = lines.iter().find(|line| line.contains("let x")).unwrap();
    assert!(code.ends_with("let x = 1;"), "{lines:?}");
    assert_eq!(lines[lines.len() - 2..], ["> first", "> second"]);
}

#[test]
fn keeps_indented_code_at_the_start_of_a_reply_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "    let x = 1;\n    let y = 2;",
        40,
    ));
    assert_eq!(lines, vec!["```", "let x = 1;", "let y = 2;", "```"]);
}

#[test]
fn keeps_indented_code_after_a_list_and_a_quote_free_of_trailing_spaces() {
    let lines = texts(&render_message(Role::Assistant, "- task\n> \n    code", 40));
    assert_eq!(lines, ["- task", "", "", "```", "code", "```"]);
}
