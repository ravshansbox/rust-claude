use super::{
    Role,
    input::{expand_tabs, input_rows},
};
use crate::highlight;
use ratatui::{
    buffer::CellWidth,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tui_markdown::{Options, StyleSheet};

mod ansi;
mod markdown;
mod theme;
mod tool;
pub(crate) use ansi::{ansi_thinking, ansi_tool_lines};
use markdown::hard_line_breaks;
pub(super) use markdown::{fence_run, next_fence};
use theme::rgb;
pub(super) use theme::{BACKGROUND, THEME, Theme, theme};
use tool::{tool_lines, tool_message_lines};

#[derive(Clone)]
struct MarkdownStyleSheet;

impl StyleSheet for MarkdownStyleSheet {
    fn code(&self) -> Style {
        match theme() {
            Theme::Light => Style::new().fg(Color::Rgb(87, 105, 247)),
            Theme::Dark => Style::new().fg(Color::Rgb(177, 185, 249)),
        }
    }
}

pub(super) fn render_message(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    let text = &highlight::strip_controls(text);
    match role {
        Role::User => user_message_lines(text, usize::from(width)),
        Role::Assistant => tui_markdown::from_str_with_options(
            &hard_line_breaks(text),
            &Options::new(MarkdownStyleSheet)
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
        Role::Event => text.lines().map(raw_line).collect(),
    }
}

pub(crate) fn tool_message(name: &str, summary: &str, diff: Option<&str>) -> String {
    match diff {
        Some(diff) => format!("{name} {summary}\n{diff}"),
        None => format!("{name} {summary}"),
    }
}

pub(crate) fn tool_failure(name: &str, error: &str) -> String {
    format!("{name} failed: {error}")
}

pub(crate) fn tool_note(name: &str, note: &str) -> String {
    format!("{name}: {note}")
}

fn thinking_style() -> Style {
    Style::new().dark_gray().italic()
}

fn raw_line(line: &str) -> Line<'static> {
    Line::raw(expand_tabs(line.to_string()))
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

pub(super) fn wrapped_height(line: &Line, width: u16) -> usize {
    Paragraph::new(borrowed_line(line))
        .wrap(Wrap { trim: false })
        .line_count(width)
}

fn user_message_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.max(1);
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
        .map(|row| {
            let padding = content_width.saturating_sub(usize::from(row.cell_width()));
            Line::from(format!("{row}{}", " ".repeat(padding))).style(theme().highlight_style())
        })
        .collect()
}

#[cfg(test)]
mod tests;
