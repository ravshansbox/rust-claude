use super::{
    Role,
    input::{expand_tabs, input_rows},
};
use crate::highlight::{self, Colour};
use pulldown_cmark::{Event, Options as ParseOptions, Parser, Tag};
use ratatui::{
    buffer::CellWidth,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tui_markdown::{Options, StyleSheet};
use unicode_segmentation::UnicodeSegmentation;

mod ansi;
mod theme;
pub(crate) use ansi::{ansi_thinking, ansi_tool_lines};
use theme::rgb;
pub(super) use theme::{BACKGROUND, THEME, Theme, theme};

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

fn hard_line_breaks(text: &str) -> String {
    let mut text = text.to_string();
    let options = ParseOptions::ENABLE_STRIKETHROUGH
        | ParseOptions::ENABLE_TASKLISTS
        | ParseOptions::ENABLE_HEADING_ATTRIBUTES
        | ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | ParseOptions::ENABLE_SUPERSCRIPT
        | ParseOptions::ENABLE_SUBSCRIPT
        | ParseOptions::ENABLE_MATH
        | ParseOptions::ENABLE_FOOTNOTES
        | ParseOptions::ENABLE_DEFINITION_LIST
        | ParseOptions::ENABLE_GFM
        | ParseOptions::ENABLE_TABLES;
    'parse: loop {
        let mut edits = Vec::new();
        for (event, range) in Parser::new_ext(&text, options).into_offset_iter() {
            match event {
                Event::Start(Tag::MetadataBlock(_)) => {
                    text.insert(range.start, '-');
                    continue 'parse;
                }
                Event::Start(Tag::Heading { .. }) => {
                    let heading = text[range.clone()].trim_end();
                    let end = range.start + heading.rfind('\n').unwrap_or(heading.len());
                    let line = text[..end].trim_end();
                    if line.ends_with('}') && !line.ends_with("\\}") {
                        edits.push((line.len() - 1, "\\"));
                    }
                }
                Event::SoftBreak => edits.push((range.start, "  ")),
                _ => {}
            }
        }
        edits.sort_unstable();
        for (offset, insert) in edits.into_iter().rev() {
            text.insert_str(offset, insert);
        }
        return text;
    }
}

/// The fence character, fence length and quote depth of the code block open
/// after `line`, given the one open before it.
pub(super) fn next_fence(
    open: Option<(char, usize, usize)>,
    line: &str,
) -> Option<(char, usize, usize)> {
    let (trimmed, depth) = match open {
        Some((_, _, depth)) => match without_quotes(line, depth) {
            (rest, found) if found == depth => (rest.trim_start(), depth),
            _ => return next_fence(None, line),
        },
        None => {
            let (rest, depth) = without_quotes(line, usize::MAX);
            let (rest, inner) = without_quotes(without_list_marker(rest.trim_start()), usize::MAX);
            (rest.trim_start(), depth + inner)
        }
    };
    let Some((fence, length)) = fence_run(trimmed) else {
        return open;
    };
    if length < 3 {
        return open;
    }
    match open {
        None if fence == '`' && trimmed[length..].contains('`') => None,
        None => Some((fence, length, depth)),
        Some((open_fence, open_length, _))
            if fence == open_fence
                && length >= open_length
                && trimmed[length..].trim().is_empty() =>
        {
            None
        }
        _ => open,
    }
}

/// The fence character `trimmed` starts with, and how many times it repeats.
pub(super) fn fence_run(trimmed: &str) -> Option<(char, usize)> {
    let fence = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    Some((fence, trimmed.chars().take_while(|c| *c == fence).count()))
}

/// `line` after up to `limit` blockquote markers, and how many it had.
fn without_quotes(line: &str, limit: usize) -> (&str, usize) {
    let mut rest = line;
    let mut depth = 0;
    while depth < limit
        && let Some(after) = rest.trim_start().strip_prefix('>')
    {
        rest = after;
        depth += 1;
    }
    (rest, depth)
}

/// `line` after a list item marker that starts it, such as `-` or `2.`.
fn without_list_marker(line: &str) -> &str {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let marker = match line[digits..].chars().next() {
        Some('.' | ')') if (1..=9).contains(&digits) => digits + 1,
        Some('-' | '*' | '+') if digits == 0 => 1,
        _ => return line,
    };
    line[marker..]
        .strip_prefix([' ', '\t'])
        .map_or(line, str::trim_start)
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

fn thinking_style() -> Style {
    Style::new().dark_gray().italic()
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
        Span::styled(format!(" {name} "), theme.highlight_style().bold()),
        Span::raw(format!(" {rest}")),
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
