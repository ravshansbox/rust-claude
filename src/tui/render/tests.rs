use super::*;
use crate::tools;

fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(ToString::to_string).collect()
}

fn trimmed(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.to_string().trim_end().to_string())
        .collect()
}

#[test]
fn wraps_user_message_by_display_width() {
    let rows = texts(&user_message_lines("日本語のテキスト\n\nabcd", 8));
    assert_eq!(rows, vec!["日本語の", "テキスト", "        ", "abcd    "]);
}

#[test]
fn shows_tab_indentation_in_user_message() {
    let rows = texts(&user_message_lines("\tx", 8));
    assert_eq!(rows, vec!["    x   "]);
}

#[test]
fn pads_user_message_with_halfwidth_sound_marks_to_width() {
    use ratatui::buffer::CellWidth;
    for line in user_message_lines("ｶﾞｷﾞ", 8) {
        assert_eq!(line.to_string().cell_width(), 8, "{line}");
    }
}

#[test]
fn shows_tab_indentation_in_replies_and_tool_output() {
    let shown = |role, text| trimmed(&render_message(role, text, 40));
    let reply = shown(
        Role::Assistant,
        "Here:\n\n```go\nfunc f() {\n\treturn\n}\n```\n\nname\tvalue",
    );
    assert!(reply.iter().any(|line| line == "    return"), "{reply:?}");
    assert!(
        reply.iter().any(|line| line == "name    value"),
        "{reply:?}"
    );
    let shell = shown(Role::Tool, "! make\n\tcc main.c");
    assert_eq!(shell[1], "    cc main.c");
    let write = shown(Role::Tool, "write main.go\n+\treturn");
    assert_eq!(write[1], "+    return");
    assert_eq!(shown(Role::Event, "\tnote"), ["    note"]);
}

#[test]
fn leaves_out_blank_lines_at_the_end_of_thinking() {
    let lines = texts(&render_message(Role::Thinking, "one\n\ntwo.\n\n", 40));
    assert_eq!(lines, ["one", "", "two."]);
}

#[test]
fn leaves_out_control_characters_from_shell_output_and_replies() {
    let tool = texts(&render_message(
        Role::Tool,
        "! ls\n\x1b[31mred\x1b[0m\r",
        20,
    ));
    assert_eq!(tool[1], format!("{:20}", "[31mred[0m"));
    for role in [Role::Assistant, Role::Thinking, Role::Event, Role::User] {
        for line in render_message(role, "hi \x1b[31mred\x07", 20) {
            let text = line.to_string();
            assert!(!text.contains(['\x1b', '\x07']), "{text:?}");
        }
    }
}

#[test]
fn highlights_bash_commands() {
    let input = serde_json::json!({ "command": "echo \"$HOME\" | wc -c" });
    let message = tool_message(
        "bash",
        &tools::summary("bash", &input),
        tools::diff("bash", &input).as_deref(),
    );
    let lines = render_message(Role::Tool, &message, 40);
    assert_eq!(trimmed(&lines), [" bash  echo \"$HOME\" | wc -c"]);
    let colours: std::collections::HashSet<_> = lines[0]
        .spans
        .iter()
        .filter_map(|span| span.style.fg)
        .collect();
    assert!(colours.len() > 1, "{:?}", lines[0]);
}

#[test]
fn keeps_later_lines_of_a_multi_line_command_below_the_tool_name() {
    let message = tool_message("bash", "", Some("cd src\nls"));
    assert_eq!(
        trimmed(&render_message(Role::Tool, &message, 40)),
        [" bash  cd src", "ls"]
    );
    let printed = ansi_tool_lines(&message, true, None, None);
    assert_eq!(printed.len(), 2, "{printed:?}");
    assert!(printed[0].contains("cd"), "{printed:?}");
}

#[test]
fn fills_tool_call_rows_with_a_background_near_the_terminal_one() {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    let text = "! make\n\tcc main.c\nthis line is longer than twenty columns";
    let lines = render_message(Role::Tool, text, 20);
    let area = Rect::new(0, 0, 20, 4);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, &mut buffer);
    let near = Color::Rgb(243, 243, 243);
    for y in 0..4 {
        let first = if y == 0 { 3 } else { 0 };
        for x in first..20 {
            assert_eq!(buffer[(x, y)].bg, near, "{x},{y} in {buffer:?}");
        }
    }
    assert_eq!(buffer[(0, 0)].bg, Color::Rgb(230, 230, 230));
}

#[test]
fn wraps_tool_output_at_spaces_and_splits_words_longer_than_a_row() {
    let text = "! ls\nsee /very/long/path/name now later";
    let rows = trimmed(&render_message(Role::Tool, text, 10));
    assert_eq!(rows[1..], ["see /very/", "long/path/", "name now", "later"]);
}

#[test]
fn keeps_colours_but_not_escapes_from_the_text_in_print_mode() {
    let text = "write notes.txt\n+a\x1b]52;c;aGk=\x07b\x1b[2J\u{9b}c\r";
    let line = &ansi_tool_lines(text, true, None, None)[1];
    assert!(line.starts_with("\x1b[0m\x1b[48;2;"), "{line:?}");
    assert!(line.ends_with("\x1b[K\x1b[0m"), "{line:?}");
    let text: String = line
        .split('\x1b')
        .map(|part| part.split_once(['m', 'K']).map_or(part, |(_, text)| text))
        .collect();
    assert_eq!(text, "+a]52;c;aGk=b[2Jc");
}

#[test]
fn shows_the_tool_name_in_bold() {
    let lines = render_message(Role::Tool, "bash ls", 20);
    assert_eq!(lines[0].spans[0].content, " bash ");
    assert!(
        lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        !lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    let line = &ansi_tool_lines("bash ls", true, None, None)[0];
    assert!(line.contains("\x1b[1m bash "), "{line:?}");
}

#[test]
fn leaves_a_print_mode_line_that_fills_its_row_uncleared() {
    let lines = ansi_tool_lines("! ls\nabcd", false, None, Some(4));
    assert!(!lines[1].contains("\x1b[K"), "{lines:?}");
    assert!(lines[0].contains("\x1b[K"), "{lines:?}");
}

#[test]
fn counts_wide_characters_moved_to_the_next_row_in_print_mode() {
    let lines = ansi_tool_lines("! ls\nabc你xy\nabc你xyz", false, None, Some(4));
    assert!(!lines[1].contains("\x1b[K"), "{lines:?}");
    assert!(lines[2].contains("\x1b[K"), "{lines:?}");
}

#[test]
fn shifts_the_terminal_background_towards_the_text() {
    let near = |theme: Theme, background| theme.tool_style(background).bg;
    assert_eq!(
        near(Theme::Dark, Some((30, 30, 30))),
        Some(Color::Rgb(42, 42, 42))
    );
    assert_eq!(
        near(Theme::Light, Some((255, 250, 5))),
        Some(Color::Rgb(243, 238, 0))
    );
    assert_eq!(near(Theme::Dark, None), Some(Color::Rgb(35, 35, 35)));
    assert_eq!(near(Theme::Light, None), Some(Color::Rgb(243, 243, 243)));
}

#[test]
fn uses_github_high_contrast_colours_in_reply_code_blocks() {
    let lines = render_message(Role::Assistant, "```python\ndef f():\n    pass\n```", 40);
    let keyword = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains("def"))
        .unwrap();
    assert_eq!(
        keyword.style.fg,
        Some(Color::Rgb(160, 17, 31)),
        "{keyword:?}"
    );
}

#[test]
fn shows_inline_code_in_blue_text() {
    let lines = render_message(Role::Assistant, "Run `cargo build` now", 40);
    let code = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == "cargo build")
        .unwrap();
    assert_eq!(code.style.fg, Some(Color::Rgb(87, 105, 247)), "{code:?}");
    assert_eq!(code.style.bg, None, "{code:?}");
}

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
fn reads_a_light_or_dark_background_from_colorfgbg() {
    let theme = |value| match Theme::from_colorfgbg(value) {
        Some(Theme::Dark) => "dark",
        Some(Theme::Light) => "light",
        None => "unknown",
    };
    assert_eq!(theme("15;0"), "dark");
    assert_eq!(theme("0;8"), "dark");
    assert_eq!(theme("0;15"), "light");
    assert_eq!(theme("0;7"), "light");
    assert_eq!(theme("12;7;0"), "dark");
    assert_eq!(theme("0;16"), "unknown");
    assert_eq!(theme("default;default"), "unknown");
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
