use super::{
    Role,
    input::{expand_tabs, input_rows},
};
use crate::highlight::{self, Colour};
use ratatui::{
    buffer::CellWidth,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use std::sync::OnceLock;
use terminal_colorsaurus::{QueryOptions, ThemeMode, color_palette};
use tui_markdown::{CodeTheme, Options, StyleSheet};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy)]
pub(super) enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub(super) fn detect() -> (Self, Option<Colour>) {
        match color_palette(QueryOptions::default()) {
            Ok(palette) => (
                match palette.theme_mode() {
                    ThemeMode::Light => Self::Light,
                    ThemeMode::Dark => Self::Dark,
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

    fn code_theme(self) -> CodeTheme {
        static DARK: OnceLock<CodeTheme> = OnceLock::new();
        static LIGHT: OnceLock<CodeTheme> = OnceLock::new();
        let load = |source| CodeTheme::from_textmate(source).expect("bundled theme");
        match self {
            Self::Light => LIGHT.get_or_init(|| load(highlight::LIGHT_THEME)),
            Self::Dark => DARK.get_or_init(|| load(highlight::DARK_THEME)),
        }
        .clone()
    }
}

pub(super) static THEME: OnceLock<Theme> = OnceLock::new();
pub(super) static BACKGROUND: OnceLock<Option<Colour>> = OnceLock::new();

pub(super) fn theme() -> Theme {
    THEME.get().copied().unwrap_or(Theme::Light)
}

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
        Role::User => user_message_lines(text, width as usize),
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
        Role::Event => text
            .lines()
            .map(|line| Line::raw(expand_tabs(line.to_string())))
            .collect(),
    }
}

fn hard_line_breaks(text: &str) -> String {
    let mut fence = None;
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let in_code = fence.is_some_and(|(_, _, depth)| without_quotes(line, depth).1 == depth);
        fence = next_fence(fence, line);
        let plain = !in_code && fence.is_none();
        if plain && line.trim_end() == "---" {
            out.push_str("----");
        } else if plain && is_heading(line) && !line.trim_end().ends_with("\\}") {
            match line.trim_end().strip_suffix('}') {
                Some(text) => {
                    out.push_str(text);
                    out.push_str("\\}");
                }
                None => out.push_str(line),
            }
        } else {
            out.push_str(line);
        }
        if plain {
            out.push_str("  ");
        }
    }
    out
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
    let Some(fence) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return open;
    };
    let length = trimmed.chars().take_while(|c| *c == fence).count();
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

fn is_heading(line: &str) -> bool {
    let (rest, depth) = without_quotes(line, usize::MAX);
    let rest = if depth > 0 {
        rest.strip_prefix(' ').unwrap_or(rest)
    } else {
        rest
    };
    if rest.starts_with('\t') || rest.starts_with("    ") {
        return false;
    }
    let rest = without_list_marker(rest.trim_start());
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    (1..=6).contains(&hashes)
        && rest[hashes..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t')
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
    match line[marker..].strip_prefix([' ', '\t']) {
        Some(rest) => rest.trim_start(),
        None => line,
    }
}

pub(crate) fn tool_message(name: &str, summary: &str, diff: Option<&str>) -> String {
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

fn highlighted_line(line: highlight::HighlightedLine, dark: bool) -> Line<'static> {
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
