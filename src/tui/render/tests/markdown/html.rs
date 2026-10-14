use super::*;

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
fn keeps_html_blocks_after_an_empty_list_item_free_of_trailing_spaces() {
    let lines = texts(&render_message(
        Role::Assistant,
        "-\n<img src=\"a.png\">\ncaption",
        40,
    ));
    assert_eq!(lines, ["- ", "", "<img src=\"a.png\">", "caption"]);
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
