use super::*;

#[test]
fn highlights_bash_commands() {
    let input = serde_json::json!({ "command": "echo \"$HOME\" | wc -c" });
    let message = tool_message(
        "bash",
        &tools::summary("bash", &input),
        tools::diff("bash", &input).as_deref(),
    );
    let lines = render_message(Role::Tool, &message, 40);
    assert_eq!(trimmed(&lines), ["bash echo \"$HOME\" | wc -c"]);
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
        ["bash cd src", "ls"]
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
    assert_eq!(lines[0].spans[0].content, "bash");
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
    assert!(line.contains("\x1b[1mbash\x1b[0m"), "{line:?}");
}

#[test]
fn leaves_a_print_mode_line_that_fills_its_row_uncleared() {
    let lines = ansi_tool_lines("! l\nabcd", false, None, Some(4));
    assert!(!lines[1].contains("\x1b[K"), "{lines:?}");
    assert!(lines[0].contains("\x1b[K"), "{lines:?}");
}

#[test]
fn counts_wide_characters_moved_to_the_next_row_in_print_mode() {
    let lines = ansi_tool_lines("! ls\nabc你xy\nabc你xyz", false, None, Some(4));
    assert!(!lines[1].contains("\x1b[K"), "{lines:?}");
    assert!(lines[2].contains("\x1b[K"), "{lines:?}");
}
