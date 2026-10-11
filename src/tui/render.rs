use super::{
    Role,
    input::{expand_tabs, input_rows},
};
use ratatui::{
    buffer::CellWidth,
    style::{Color, Style, Stylize},
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
    pub(super) fn detect() -> Self {
        match terminal_colorsaurus::theme_mode(terminal_colorsaurus::QueryOptions::default()) {
            Ok(terminal_colorsaurus::ThemeMode::Light) => Self::Light,
            Ok(terminal_colorsaurus::ThemeMode::Dark) => Self::Dark,
            Err(_) => std::env::var("COLORFGBG")
                .ok()
                .and_then(|value| Self::from_colorfgbg(&value))
                .unwrap_or(Self::Light),
        }
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
    match role {
        Role::User => user_message_lines(text, width as usize),
        Role::Assistant => tui_markdown::from_str_with_options(
            &hard_line_breaks(text),
            &tui_markdown::Options::new(MarkdownStyleSheet)
                .code_theme(theme().code_theme())
                .table_width(width.saturating_sub(INDENT_WIDTH)),
        )
        .lines
        .into_iter()
        .map(owned_line)
        .collect(),
        Role::Thinking => text
            .lines()
            .map(|line| Line::from(expand_tabs(line.to_string()).dark_gray().italic()))
            .collect(),
        Role::Tool => tool_message_lines(text),
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

pub(super) fn tool_message(name: &str, summary: String, diff: Option<String>) -> String {
    match diff {
        Some(diff) => format!("{name} {summary}\n{diff}"),
        None => format!("{name} {summary}"),
    }
}

fn tool_message_lines(text: &str) -> Vec<Line<'static>> {
    let Some((first, body)) = text.split_once('\n') else {
        return vec![tool_header(text)];
    };
    let (name, path) = first.split_once(' ').unwrap_or((first, ""));
    let mut lines = vec![tool_header(first)];
    let dark = matches!(theme(), Theme::Dark);
    if let Some(highlighted) = crate::highlight::highlight_tool(name, path, body, dark) {
        lines.extend(
            highlighted
                .into_iter()
                .map(|line| highlighted_line(line, dark)),
        );
    } else {
        lines.extend(
            body.lines()
                .map(|line| Line::raw(expand_tabs(line.to_string()))),
        );
    }
    lines
}

fn tool_header(first: &str) -> Line<'static> {
    let (name, rest) = match first.split_once("] ") {
        Some((name, rest)) if name.starts_with('[') => (&first[..=name.len()], rest),
        _ => first.split_once(' ').unwrap_or((first, "")),
    };
    Line::from(vec![
        Span::styled(format!(" {name} "), theme().highlight_style()),
        Span::raw(format!(" {rest}")),
    ])
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

pub(super) const REPLY_MARKER: &str = "● ";
pub(super) const INDENT: &str = "  ";
const INDENT_WIDTH: u16 = 2;

/// The line wrapped at word boundaries to fit after a two-column prefix:
/// `first_prefix` on the first row and `INDENT` on the rest.
pub(super) fn indented_rows(
    line: &Line<'_>,
    width: u16,
    first_prefix: &'static str,
) -> Vec<Line<'static>> {
    let width = usize::from(width.saturating_sub(INDENT_WIDTH).max(1));
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
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let prefix = if index == 0 { first_prefix } else { INDENT };
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (grapheme, style) in row {
                match spans.last_mut() {
                    Some(span) if span.style == style => span.content.to_mut().push_str(grapheme),
                    _ => spans.push(Span::styled(grapheme.to_string(), style)),
                }
            }
            spans.insert(0, Span::raw(prefix));
            Line::from(spans).style(line.style)
        })
        .collect()
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

    #[test]
    fn wraps_user_message_by_display_width() {
        let rows: Vec<String> = user_message_lines("日本語のテキスト\n\nabcd", 8)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(
            rows,
            vec!["❯ 日本語", "  のテキ", "  スト  ", "        ", "  abcd  ",]
        );
    }

    #[test]
    fn shows_tab_indentation_in_user_message() {
        let rows: Vec<String> = user_message_lines("\tx", 8)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(rows, vec!["❯     x "]);
    }

    #[test]
    fn wraps_reply_rows_by_display_width_and_breaks_long_words() {
        let rows = |text: &str| -> Vec<String> {
            indented_rows(&Line::raw(text.to_string()), 8, REPLY_MARKER)
                .iter()
                .map(|line| line.to_string())
                .collect()
        };
        assert_eq!(rows("日本語のテキ"), ["● 日本語", "  のテキ"]);
        assert_eq!(rows("ab abcdefgh"), ["● ab abc", "  defgh"]);
        assert_eq!(rows("abc   def"), ["● abc   ", "  def"]);
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
        let shown = |role, text| -> Vec<String> {
            render_message(role, text, 40)
                .iter()
                .map(|line| line.to_string().trim_end().to_string())
                .collect()
        };
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
    fn highlights_bash_commands() {
        let input = serde_json::json!({ "command": "echo \"$HOME\" | wc -c" });
        let message = tool_message(
            "bash",
            crate::tools::summary("bash", &input),
            crate::tools::diff("bash", &input),
        );
        let lines = render_message(Role::Tool, &message, 40);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| line.to_string().trim_end().to_string())
            .collect();
        assert_eq!(texts, [" bash", " echo \"$HOME\" | wc -c"]);
        let colours: std::collections::HashSet<_> = lines[1]
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .collect();
        assert!(colours.len() > 1, "{:?}", lines[1]);
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
