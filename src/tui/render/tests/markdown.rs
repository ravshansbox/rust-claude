use super::*;

#[test]
fn fits_assistant_table_to_width() {
    let text = "| Name | Description |\n| --- | --- |\n| alpha | a fairly long description that needs wrapping |";
    let lines = render_message(Role::Assistant, text, 30);
    assert!(lines.iter().any(|line| line.to_string().contains('│')));
    assert!(lines.iter().all(|line| line.width() <= 30));
}

#[test]
fn keeps_line_breaks_after_a_code_block_holding_another_fence() {
    let text = "```\n~~~\n```\nfirst\nsecond";
    let lines: Vec<String> = render_message(Role::Assistant, text, 40)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(lines.iter().any(|line| line.trim() == "first"), "{lines:?}");
    assert!(
        lines.iter().any(|line| line.trim() == "second"),
        "{lines:?}"
    );
}

#[test]
fn keeps_line_breaks_after_inline_code_in_triple_backticks() {
    let text = "```ls```\nfirst\nsecond";
    let lines: Vec<String> = render_message(Role::Assistant, text, 40)
        .iter()
        .map(|line| line.to_string().trim().to_string())
        .collect();
    assert_eq!(lines, ["ls", "first", "second"]);
}

#[test]
fn keeps_line_breaks_after_a_code_block_opened_on_a_list_item_line() {
    let text = "1. ```sh\n   pwd\n   ```\n\nfirst\nsecond";
    let lines = trimmed(&render_message(Role::Assistant, text, 40));
    assert!(lines.iter().any(|line| line == "pwd"), "{lines:?}");
    assert_eq!(lines[lines.len() - 2..], ["first", "second"]);
}

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
fn shows_markdown_between_two_rules_as_markdown() {
    let lines = trimmed(&render_message(
        Role::Assistant,
        "Intro\n\n---\n## Part *one*\nMore *text*\n\n---\n\nEnd",
        40,
    ));
    assert_eq!(
        lines,
        [
            "Intro",
            "",
            "---",
            "",
            "## Part one",
            "",
            "More text",
            "",
            "---",
            "",
            "End"
        ]
    );
}

#[test]
fn shows_braces_at_the_end_of_a_heading() {
    let lines = trimmed(&render_message(
        Role::Assistant,
        "## Point { x, y }\n\n> ### Empty {}\n\n- # Item {a}",
        40,
    ));
    assert_eq!(
        lines,
        [
            "## Point { x, y }",
            "",
            "> ### Empty {}",
            "",
            "-",
            "# Item {a}"
        ]
    );
}

#[test]
fn shows_braces_in_an_indented_code_block_as_they_are() {
    let lines = trimmed(&render_message(
        Role::Assistant,
        "Intro\n\n    # use ${HOME}\n\nEnd",
        40,
    ));
    assert_eq!(
        lines,
        ["Intro", "", "```", "# use ${HOME}", "```", "", "End"]
    );
}

#[test]
fn shows_braces_at_the_end_of_an_underlined_heading() {
    let lines = trimmed(&render_message(
        Role::Assistant,
        "Point { x, y }\n===\n\n> Empty {}\n> ---",
        40,
    ));
    assert_eq!(lines, ["# Point { x, y }", "", "> ## Empty {}"]);
}

#[test]
fn hides_the_backslash_of_a_hard_line_break() {
    let lines = trimmed(&render_message(
        Role::Assistant,
        "Line one\\\nLine two \\\\\nLine three",
        40,
    ));
    assert_eq!(lines, ["Line one", "Line two \\", "Line three"]);
}

#[test]
fn joins_inline_code_split_across_lines_with_one_space() {
    let lines = render_message(
        Role::Assistant,
        "Run `cargo\ntest` now\nand `this\nthat",
        80,
    );
    assert_eq!(trimmed(&lines), ["Run cargo test now", "and `this", "that"]);
}

#[test]
fn keeps_html_blocks_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "<details>\n<summary>More</summary>\nHidden\n</details>\n\n<!--\nnote\n-->\nAfter\nnext",
        40,
    ));
    assert_eq!(
        lines,
        [
            "<details>",
            "<summary>More</summary>",
            "Hidden",
            "</details>",
            "",
            "<!--",
            "note",
            "-->",
            "",
            "After",
            "next"
        ]
    );
}

#[test]
fn ends_a_quoted_html_block_when_the_quote_ends() {
    let lines = texts(&render_message(
        Role::Assistant,
        "> <div>\n> a\nplain\nline",
        40,
    ));
    assert_eq!(lines, ["> <div>", "> a", "", "plain", "line"]);
}

#[test]
fn keeps_html_blocks_after_an_empty_quote_line_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "> tip\n>\n<img src=\"a.png\">\ncaption",
        40,
    ));
    assert_eq!(lines, ["> tip", "", "<img src=\"a.png\">", "caption"]);
}

#[test]
fn keeps_html_blocks_with_quoted_angle_brackets_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "<a href=\"x>y\">\nlink\n</a>\n\n<b title='1 > 0' hidden>\ntext",
        40,
    ));
    assert_eq!(
        lines,
        [
            "<a href=\"x>y\">",
            "link",
            "</a>",
            "",
            "<b title='1 > 0' hidden>",
            "text"
        ]
    );
}

#[test]
fn keeps_lines_with_invalid_tags_as_paragraph_text() {
    let lines = texts(&render_message(Role::Assistant, "<foo bar=>\nnext", 40));
    assert_eq!(lines, ["<foo bar=>", "next"]);
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
fn keeps_math_split_across_lines_free_of_added_spaces() {
    let display = texts(&render_message(
        Role::Assistant,
        "$$\nE = mc^2\n$$\nnext\nline",
        40,
    ));
    assert_eq!(display, vec!["$$", "E = mc^2", "$$", "", "next", "line"]);
    let inline = texts(&render_message(
        Role::Assistant,
        "Energy $E = \nmc^2$ here\nnext",
        40,
    ));
    assert_eq!(inline, vec!["Energy $E = mc^2$ here", "next"]);
    let prices = texts(&render_message(
        Role::Assistant,
        "Costs $5 and\n$6 total\nnext",
        40,
    ));
    assert_eq!(prices, vec!["Costs $5 and", "$6 total", "next"]);
}

#[test]
fn keeps_html_blocks_after_an_empty_list_item_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "-\n<img src=\"a.png\">\ncaption",
        40,
    ));
    assert_eq!(lines, ["- ", "", "<img src=\"a.png\">", "caption"]);
}

#[test]
fn keeps_indented_code_after_a_list_and_a_quote_free_of_trailing_spaces() {
    let lines = texts(&render_message(Role::Assistant, "- task\n> \n    code", 40));
    assert_eq!(lines, ["- task", "", "", "```", "code", "```"]);
}

#[test]
fn keeps_html_blocks_open_across_a_lone_quote_marker() {
    let lines = texts(&render_message(
        Role::Assistant,
        "<div>\n>\nnote\n</div>",
        40,
    ));
    assert_eq!(lines, ["<div>", ">", "note", "</div>"]);
}

#[test]
fn keeps_line_breaks_in_a_list_item_after_an_unclosed_backtick() {
    let lines = texts(&render_message(Role::Assistant, "`a\n- item\n`b", 40));
    assert_eq!(lines, ["`a", "", "- item", "`b"]);
}

#[test]
fn keeps_line_breaks_in_a_quote_after_an_unclosed_backtick() {
    let lines = texts(&render_message(Role::Assistant, "`a\n> quote\n> `b", 40));
    assert_eq!(lines, ["`a", "", "> quote", "> `b"]);
}

#[test]
fn keeps_inline_code_across_an_ordered_marker_that_cannot_start_a_list() {
    let lines = texts(&render_message(Role::Assistant, "`a\n2. b`", 40));
    assert_eq!(lines, ["a 2. b"]);
}
