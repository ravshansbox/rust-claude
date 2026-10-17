use super::{BACKGROUND, Theme, expand_tabs, raw_line, rgb, theme};
use crate::highlight::{self, Colour};
use ratatui::{
    buffer::CellWidth,
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn tool_message_lines(text: &str, width: u16) -> Vec<Line<'static>> {
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

/// The tool call's header and body, each line with the tool background.
pub(super) fn tool_lines(
    text: &str,
    theme: Theme,
    background: Option<Colour>,
) -> Vec<Line<'static>> {
    let (first, body) = match text.split_once('\n') {
        Some((first, body)) => (first, Some(body)),
        None => (text, None),
    };
    let (name, path) = first.split_once(' ').unwrap_or((first, ""));
    let (header, joins) = tool_header(first, theme);
    let mut lines = vec![header];
    let dark = matches!(theme, Theme::Dark);
    if let Some(body) = body {
        match highlight::highlight_tool(name, path, body, dark) {
            Some(highlighted) => lines.extend(
                highlighted
                    .into_iter()
                    .map(|line| highlighted_line(line, dark)),
            ),
            None => lines.extend(body.lines().map(raw_line)),
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
        Span::styled(
            expand_tabs(name.to_string()),
            theme.highlight_style().bold(),
        ),
        Span::raw(expand_tabs(format!(" {rest}"))),
    ]);
    (line, rest.is_empty())
}

fn highlighted_line(line: highlight::HighlightedLine, dark: bool) -> Line<'static> {
    let background = line.background(dark);
    let spans: Vec<Span<'static>> = line
        .segments
        .into_iter()
        .map(|segment| {
            let style = match segment.foreground {
                Some(colour) => Style::new().fg(rgb(colour)),
                None => Style::new(),
            };
            Span::styled(expand_tabs(segment.text), style)
        })
        .collect();
    let line = Line::from(spans);
    match background {
        Some(colour) => line.style(Style::new().bg(rgb(colour))),
        None => line,
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
