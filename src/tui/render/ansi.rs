use super::{Theme, expand_tabs, thinking_style, tool_lines};
use crate::highlight::{self, Colour};
use ratatui::{
    buffer::CellWidth,
    style::{Color, Modifier, Style},
    text::Line,
};
use unicode_segmentation::UnicodeSegmentation;

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

/// Part of the thinking with ANSI codes for its style, for print mode.
pub(crate) fn ansi_thinking(text: &str) -> String {
    let text = highlight::strip_controls(text);
    let mut ansi = String::new();
    push_sgr(&mut ansi, thinking_style());
    ansi.push_str(&expand_tabs(text.into_owned()));
    ansi.push_str("\x1b[0m");
    ansi
}

fn push_sgr(codes: &mut String, style: Style) {
    use std::fmt::Write;
    if let Some(Color::Rgb(red, green, blue)) = style.bg {
        let _ = write!(codes, "\x1b[48;2;{red};{green};{blue}m");
    }
    match style.fg {
        Some(Color::Rgb(red, green, blue)) => {
            let _ = write!(codes, "\x1b[38;2;{red};{green};{blue}m");
        }
        Some(Color::DarkGray) => codes.push_str("\x1b[90m"),
        _ => {}
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push_str("\x1b[1m");
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push_str("\x1b[3m");
    }
}

fn ansi_line(line: &Line, columns: Option<u16>) -> String {
    let mut spans: Vec<(String, Style)> = line
        .spans
        .iter()
        .map(|span| {
            let text = highlight::strip_controls(&span.content).into_owned();
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
    for (text, style) in &spans {
        ansi.push_str("\x1b[0m");
        push_sgr(&mut ansi, *style);
        ansi.push_str(text);
    }
    push_sgr(
        &mut ansi,
        Style::new().bg(line.style.bg.unwrap_or(Color::Reset)),
    );
    let fills_row = columns.is_some_and(|columns| {
        let columns = usize::from(columns.max(1));
        let mut column = 0;
        for grapheme in spans.iter().flat_map(|(text, _)| text.graphemes(true)) {
            let width = usize::from(grapheme.cell_width());
            if column + width > columns {
                column = 0;
            }
            column += width;
        }
        column == columns
    });
    if !fills_row {
        ansi.push_str("\x1b[K");
    }
    ansi.push_str("\x1b[0m");
    ansi
}
