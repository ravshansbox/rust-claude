use super::{
    Role,
    input::{expand_tabs, input_rows},
};
use crate::highlight::Colour;
use ratatui::{
    buffer::CellWidth,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy)]
pub(super) enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub(super) fn detect() -> (Self, Option<Colour>) {
        match terminal_colorsaurus::color_palette(terminal_colorsaurus::QueryOptions::default()) {
            Ok(palette) => (
                match palette.theme_mode() {
                    terminal_colorsaurus::ThemeMode::Light => Self::Light,
                    terminal_colorsaurus::ThemeMode::Dark => Self::Dark,
                },
                Some(palette.background.scale_to_8bit()),
            ),
            Err(_) => (
                std::env::var("COLORFGBG")
                    .ok()
                    .and_then(|value| Self::from_colorfgbg(&value))
                    .unwrap_or(Self::Light),
                None,
            ),
        }
    }

    fn near_background(self, background: Option<Colour>) -> Colour {
        let shift = |channel: u8| match self {
            Self::Light => channel.saturating_sub(12),
            Self::Dark => channel.saturating_add(12),
        };
        let (red, green, blue) = background.unwrap_or(match self {
            Self::Light => (255, 255, 255),
            Self::Dark => (23, 23, 23),
        });
        (shift(red), shift(green), shift(blue))
    }

    fn tool_style(self, background: Option<Colour>) -> Style {
        let (red, green, blue) = self.near_background(background);
        Style::new().bg(Color::Rgb(red, green, blue))
    }

    fn from_colorfgbg(value: &str) -> Option<Self> {
        match value.rsplit(';').next()?.parse::<u8>().ok()? {
            0..=6 | 8 => Some(Self::Dark),
            7 | 9..=15 => Some(Self::Light),
            _ => None,
        }
    }

    fn highlight_style(self) -> Style {
        match self {
            Self::Light => Style::new()
                .fg(Color::Rgb(62, 62, 62))
                .bg(Color::Rgb(230, 230, 230)),
            Self::Dark => Style::new()
                .fg(Color::Rgb(220, 220, 220))
                .bg(Color::Rgb(50, 50, 50)),
        }
    }

    pub(super) fn subtle_style(self) -> Style {
        match self {
            Self::Light => Style::new().bg(Color::Rgb(245, 245, 245)),
            Self::Dark => Style::new().bg(Color::Rgb(35, 35, 35)),
        }
    }

    fn code_theme(self) -> tui_markdown::CodeTheme {
        static DARK: std::sync::OnceLock<tui_markdown::CodeTheme> = std::sync::OnceLock::new();
        static LIGHT: std::sync::OnceLock<tui_markdown::CodeTheme> = std::sync::OnceLock::new();
        let load = |source| tui_markdown::CodeTheme::from_textmate(source).expect("bundled theme");
        match self {
            Self::Light => LIGHT.get_or_init(|| load(crate::highlight::LIGHT_THEME)),
            Self::Dark => DARK.get_or_init(|| load(crate::highlight::DARK_THEME)),
        }
        .clone()
    }
}

pub(super) static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
pub(super) static BACKGROUND: std::sync::OnceLock<Option<Colour>> = std::sync::OnceLock::new();

pub(super) fn theme() -> Theme {
    THEME.get().copied().unwrap_or(Theme::Light)
}

#[derive(Clone)]
struct MarkdownStyleSheet;

impl tui_markdown::StyleSheet for MarkdownStyleSheet {
    fn code(&self) -> Style {
        match theme() {
            Theme::Light => Style::new().fg(Color::Rgb(87, 105, 247)),
            Theme::Dark => Style::new().fg(Color::Rgb(177, 185, 249)),
        }
    }
}

pub(super) fn render_message(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    let text = &crate::highlight::strip_controls(text);
    match role {
        Role::User => user_message_lines(text, width as usize),
        Role::Assistant => tui_markdown::from_str_with_options(
            &hard_line_breaks(text),
            &tui_markdown::Options::new(MarkdownStyleSheet)
                .code_theme(theme().code_theme())
                .table_width(width),
        )
        .lines
        .into_iter()
        .map(owned_line)
        .collect(),
        Role::Thinking => text
            .trim_end_matches('\n')
            .lines()
            .map(|line| Line::styled(expand_tabs(line.to_string()), thinking_style()))
            .collect(),
        Role::Tool => tool_message_lines(text, width),
        Role::Event => text
            .lines()
            .map(|line| Line::raw(expand_tabs(line.to_string())))
            .collect(),
    }
}

fn hard_line_breaks(text: &str) -> String {
    let mut in_fence = false;
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                line.to_string()
            } else if in_fence {
                line.to_string()
            } else {
                format!("{line}  ")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn tool_message(name: &str, summary: String, diff: Option<String>) -> String {
    match diff {
        Some(diff) => format!("{name} {summary}\n{diff}"),
        None => format!("{name} {summary}"),
    }
}

fn tool_message_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    tool_lines(text, theme(), BACKGROUND.get().copied().flatten())
        .iter()
        .flat_map(|line| {
            wrapped_rows(line, width).into_iter().map(move |row| {
                let used: usize = row
                    .iter()
                    .map(|(grapheme, _)| usize::from(grapheme.cell_width()))
                    .sum();
                let mut spans = joined_spans(row);
                spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
                Line::from(spans).style(line.style)
            })
        })
        .collect()
}

/// A tool call as lines with ANSI colour codes, for print mode. Each line
/// ends by filling the rest of its row with its background, except a line
/// that fills its last row, where clearing would remove the last character.
pub(crate) fn ansi_tool_lines(
    text: &str,
    dark: bool,
    background: Option<Colour>,
    columns: Option<u16>,
) -> Vec<String> {
    let theme = if dark { Theme::Dark } else { Theme::Light };
    tool_lines(text, theme, background)
        .iter()
        .map(|line| ansi_line(line, columns))
        .collect()
}

fn thinking_style() -> Style {
    Style::new().dark_gray().italic()
}

/// Part of the thinking with ANSI codes for its style, for print mode.
pub(crate) fn ansi_thinking(text: &str) -> String {
    let text = crate::highlight::strip_controls(text);
    format!(
        "{}{}\x1b[0m",
        sgr(thinking_style()),
        expand_tabs(text.into_owned())
    )
}

fn sgr(style: Style) -> String {
    let mut codes = String::new();
    if let Some(Color::Rgb(red, green, blue)) = style.bg {
        codes.push_str(&format!("\x1b[48;2;{red};{green};{blue}m"));
    }
    match style.fg {
        Some(Color::Rgb(red, green, blue)) => {
            codes.push_str(&format!("\x1b[38;2;{red};{green};{blue}m"))
        }
        Some(Color::DarkGray) => codes.push_str("\x1b[90m"),
        _ => {}
    }
    if style.add_modifier.contains(ratatui::style::Modifier::BOLD) {
        codes.push_str("\x1b[1m");
    }
    if style
        .add_modifier
        .contains(ratatui::style::Modifier::ITALIC)
    {
        codes.push_str("\x1b[3m");
    }
    codes
}

fn ansi_line(line: &Line, columns: Option<u16>) -> String {
    let mut spans: Vec<(String, Style)> = line
        .spans
        .iter()
        .map(|span| {
            let text = crate::highlight::strip_controls(&span.content).into_owned();
            (text, line.style.patch(span.style))
        })
        .collect();
    while let Some((text, style)) = spans.last_mut()
        && style.bg == line.style.bg
    {
        let trimmed = text.trim_end().len();
        text.truncate(trimmed);
        if !text.is_empty() {
            break;
        }
        spans.pop();
    }
    let mut ansi = String::new();
    let mut width = 0;
    for (text, style) in &spans {
        ansi.push_str("\x1b[0m");
        ansi.push_str(&sgr(*style));
        ansi.push_str(text);
        width += usize::from(text.as_str().cell_width());
    }
    ansi.push_str(&sgr(Style::new().bg(line.style.bg.unwrap_or(Color::Reset))));
    let fills_row =
        columns.is_some_and(|columns| width > 0 && width % usize::from(columns.max(1)) == 0);
    if !fills_row {
        ansi.push_str("\x1b[K");
    }
    ansi.push_str("\x1b[0m");
    ansi
}

/// The tool call's header and body, each line with the tool background.
fn tool_lines(text: &str, theme: Theme, background: Option<Colour>) -> Vec<Line<'static>> {
    let (first, body) = match text.split_once('\n') {
        Some((first, body)) => (first, Some(body)),
        None => (text, None),
    };
    let (name, path) = first.split_once(' ').unwrap_or((first, ""));
    let (header, joins) = tool_header(first, theme);
    let mut lines = vec![header];
    let dark = matches!(theme, Theme::Dark);
    if let Some(body) = body {
        match crate::highlight::highlight_tool(name, path, body, dark) {
            Some(highlighted) => lines.extend(
                highlighted
                    .into_iter()
                    .map(|line| highlighted_line(line, dark)),
            ),
            None => lines.extend(
                body.lines()
                    .map(|line| Line::raw(expand_tabs(line.to_string()))),
            ),
        }
    }
    if joins && lines.len() > 1 {
        let next = lines.remove(1);
        lines[0].spans.extend(next.spans);
    }
    let style = theme.tool_style(background);
    lines
        .into_iter()
        .map(|line| {
            let line_style = style.patch(line.style);
            line.style(line_style)
        })
        .collect()
}

fn tool_header(first: &str, theme: Theme) -> (Line<'static>, bool) {
    let (name, rest) = match first.split_once("] ") {
        Some((name, rest)) if name.starts_with('[') => (&first[..=name.len()], rest),
        _ => first.split_once(' ').unwrap_or((first, "")),
    };
    let line = Line::from(vec![
        Span::styled(format!(" {name} "), theme.highlight_style().bold()),
        Span::raw(format!(" {rest}")),
    ]);
    (line, rest.is_empty())
}

fn highlighted_line(line: crate::highlight::HighlightedLine, dark: bool) -> Line<'static> {
    let background = line.background(dark);
    let spans: Vec<Span<'static>> = line
        .segments
        .into_iter()
        .map(|segment| {
            let style = match segment.foreground {
                Some((red, green, blue)) => Style::new().fg(Color::Rgb(red, green, blue)),
                None => Style::new(),
            };
            Span::styled(expand_tabs(segment.text), style)
        })
        .collect();
    let line = Line::from(spans);
    match background {
        Some((red, green, blue)) => line.style(Style::new().bg(Color::Rgb(red, green, blue))),
        None => line,
    }
}

/// The line with its own text, and tabs shown as spaces.
fn owned_line(line: Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .into_iter()
            .map(|span| Span::styled(expand_tabs(span.content.into_owned()), span.style))
            .collect(),
    }
}

pub(super) fn borrowed_line<'a>(line: &'a Line<'_>) -> Line<'a> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|span| Span::styled(span.content.as_ref(), span.style))
            .collect(),
    }
}

/// The line's graphemes and their styles in rows of at most `width`
/// columns, broken at word boundaries where a word fits on a row.
fn wrapped_rows<'a>(line: &'a Line<'_>, width: usize) -> Vec<Vec<(&'a str, Style)>> {
    let graphemes: Vec<(&str, Style)> = line
        .spans
        .iter()
        .flat_map(|span| {
            span.content
                .graphemes(true)
                .map(move |grapheme| (grapheme, span.style))
        })
        .collect();
    let is_space = |grapheme: &str| grapheme.chars().all(char::is_whitespace);
    let mut rows: Vec<Vec<(&str, Style)>> = vec![Vec::new()];
    let mut row_width = 0;
    let mut start = 0;
    while start < graphemes.len() {
        let space = is_space(graphemes[start].0);
        let end = (start..graphemes.len())
            .find(|&index| is_space(graphemes[index].0) != space)
            .unwrap_or(graphemes.len());
        let word = &graphemes[start..end];
        let word_width: usize = word
            .iter()
            .map(|(grapheme, _)| usize::from(grapheme.cell_width()))
            .sum();
        start = end;
        if space && row_width == 0 && rows.len() > 1 {
            continue;
        }
        if !space && row_width > 0 && row_width + word_width > width && word_width <= width {
            rows.push(Vec::new());
            row_width = 0;
        }
        for &(grapheme, style) in word {
            let grapheme_width = usize::from(grapheme.cell_width());
            if row_width + grapheme_width > width {
                if space {
                    break;
                }
                if row_width > 0 {
                    rows.push(Vec::new());
                    row_width = 0;
                }
            }
            if let Some(row) = rows.last_mut() {
                row.push((grapheme, style));
            }
            row_width += grapheme_width;
        }
    }
    rows
}

fn joined_spans(row: Vec<(&str, Style)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (grapheme, style) in row {
        match spans.last_mut() {
            Some(span) if span.style == style => span.content.to_mut().push_str(grapheme),
            _ => spans.push(Span::styled(grapheme.to_string(), style)),
        }
    }
    spans
}

pub(super) fn wrapped_height(line: &Line, width: u16) -> usize {
    Paragraph::new(borrowed_line(line))
        .wrap(Wrap { trim: false })
        .line_count(width)
}

fn user_message_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(2).max(1);
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut line_rows = input_rows(line, content_width);
        if line_rows.len() > 1 && line_rows.last().is_some_and(String::is_empty) {
            line_rows.pop();
        }
        rows.extend(line_rows);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let marker = if index == 0 { "❯ " } else { "  " };
            let padding = content_width.saturating_sub(usize::from(row.cell_width()));
            Line::from(format!("{marker}{row}{}", " ".repeat(padding)))
                .style(theme().highlight_style())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
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
        assert_eq!(
            rows,
            vec!["❯ 日本語", "  のテキ", "  スト  ", "        ", "  abcd  ",]
        );
    }

    #[test]
    fn shows_tab_indentation_in_user_message() {
        let rows = texts(&user_message_lines("\tx", 8));
        assert_eq!(rows, vec!["❯     x "]);
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
            crate::tools::summary("bash", &input),
            crate::tools::diff("bash", &input),
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
        let message = tool_message("bash", String::new(), Some("cd src\nls".into()));
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
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert!(
            !lines[0].spans[1]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
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
    fn shifts_the_terminal_background_towards_the_text() {
        assert_eq!(
            Theme::Dark.near_background(Some((30, 30, 30))),
            (42, 42, 42)
        );
        assert_eq!(
            Theme::Light.near_background(Some((255, 250, 5))),
            (243, 238, 0)
        );
        assert_eq!(Theme::Dark.near_background(None), (35, 35, 35));
        assert_eq!(Theme::Light.near_background(None), (243, 243, 243));
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
}
