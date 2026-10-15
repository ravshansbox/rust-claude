use super::*;
use crate::tools;
use ratatui::style::Modifier;

mod markdown;
mod tool_calls;

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
