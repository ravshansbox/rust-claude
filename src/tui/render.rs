use super::{Role, input::input_rows};
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

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

    fn code_theme(self) -> tui_markdown::BuiltinCodeTheme {
        match self {
            Self::Light => tui_markdown::BuiltinCodeTheme::Base16OceanLight,
            Self::Dark => tui_markdown::BuiltinCodeTheme::Base16OceanDark,
        }
    }
}

pub(super) static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();

fn theme() -> Theme {
    THEME.get().copied().unwrap_or(Theme::Light)
}

#[derive(Clone)]
struct MarkdownStyleSheet;

impl tui_markdown::StyleSheet for MarkdownStyleSheet {
    fn code(&self) -> Style {
        theme().highlight_style()
    }
}

pub(super) fn render_message(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    match role {
        Role::User => user_message_lines(text, width as usize),
        Role::Assistant => tui_markdown::from_str_with_options(
            &hard_line_breaks(text),
            &tui_markdown::Options::new(MarkdownStyleSheet).code_theme(theme().code_theme()),
        )
        .lines
        .into_iter()
        .map(owned_line)
        .collect(),
        Role::Thinking => text
            .lines()
            .map(|line| Line::from(line.to_string().dark_gray().italic()))
            .collect(),
        Role::Tool => tool_message_lines(text),
        Role::Event => text
            .lines()
            .map(|line| Line::raw(line.to_string()))
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
    let is_edit = text.starts_with("edit ");
    let mut lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| match line.chars().next() {
            Some('-') if is_edit => Line::from(line.to_string().red()),
            Some('+') if is_edit => Line::from(line.to_string().green()),
            _ => Line::raw(line.to_string()),
        })
        .collect();
    if let Some(first) = text.lines().next() {
        let (name, rest) = first.split_once(' ').unwrap_or((first, ""));
        lines[0] = Line::from(vec![
            Span::styled(format!(" {name} "), theme().highlight_style()),
            Span::raw(format!(" {rest}")),
        ]);
    }
    lines
}

fn owned_line(line: Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect(),
    }
}

pub(super) fn borrowed_line<'a>(line: &'a Line<'static>) -> Line<'a> {
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

fn user_message_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(2).max(1);
    let mut rows = vec![String::new()];
    for line in text.lines() {
        let mut line_rows = input_rows(line, content_width);
        if line_rows.len() > 1 && line_rows.last().is_some_and(String::is_empty) {
            line_rows.pop();
        }
        rows.extend(line_rows);
    }
    rows.push(String::new());
    rows.into_iter()
        .map(|row| {
            let padding = content_width.saturating_sub(row.width()) + 1;
            Line::from(format!(" {row}{}", " ".repeat(padding))).style(theme().highlight_style())
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
            vec![
                "        ",
                " 日本語 ",
                " のテキ ",
                " スト   ",
                "        ",
                " abcd   ",
                "        ",
            ]
        );
    }
}
