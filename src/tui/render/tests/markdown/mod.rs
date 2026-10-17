use super::*;

mod html;
mod trailing_spaces;

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

#[test]
fn keeps_table_borders_aligned_when_a_cell_holds_a_tab() {
    let lines = render_message(Role::Assistant, "| a\tb | c |\n|---|---|\n| d | e |", 40);
    let widths: Vec<usize> = lines.iter().map(Line::width).collect();
    assert!(widths.iter().all(|width| *width == widths[0]), "{lines:?}");
}
