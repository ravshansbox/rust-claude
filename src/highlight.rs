use std::{borrow::Cow, io::Cursor, sync::OnceLock};

use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

mod embedded;

use embedded::embedded_code;

pub type Colour = (u8, u8, u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Delete,
    Insert,
    Equal,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Segment {
    pub foreground: Option<Colour>,
    pub text: String,
}

impl Segment {
    fn plain(text: &str) -> Self {
        Self {
            foreground: None,
            text: text.to_string(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct HighlightedLine {
    pub change: Option<Change>,
    pub segments: Vec<Segment>,
}

impl HighlightedLine {
    pub fn background(&self, dark: bool) -> Option<Colour> {
        match (self.change?, dark) {
            (Change::Delete, false) => Some((255, 225, 225)),
            (Change::Insert, false) => Some((220, 245, 220)),
            (Change::Delete, true) => Some((75, 30, 30)),
            (Change::Insert, true) => Some((30, 65, 30)),
            (Change::Equal, _) => None,
        }
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_nonewlines)
}

pub const DARK_THEME: &str = include_str!("../assets/themes/GitHub Dark High Contrast.tmTheme");
pub const LIGHT_THEME: &str = include_str!("../assets/themes/GitHub Light High Contrast.tmTheme");

fn theme(dark: bool) -> &'static Theme {
    static DARK: OnceLock<Theme> = OnceLock::new();
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    let load =
        |source: &str| ThemeSet::load_from_reader(&mut Cursor::new(source)).expect("bundled theme");
    if dark {
        DARK.get_or_init(|| load(DARK_THEME))
    } else {
        LIGHT.get_or_init(|| load(LIGHT_THEME))
    }
}

fn syntax_for_path(path: &str) -> &'static SyntaxReference {
    let syntax_set = syntax_set();
    let path = std::path::Path::new(path);
    [path.extension(), path.file_name()]
        .into_iter()
        .flatten()
        .find_map(|name| syntax_set.find_syntax_by_extension(name.to_str()?))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn change_for_sign(sign: char) -> Option<Change> {
    match sign {
        '-' => Some(Change::Delete),
        '+' => Some(Change::Insert),
        ' ' => Some(Change::Equal),
        _ => None,
    }
}

fn highlight_code(highlighter: &mut HighlightLines, code: &str) -> Vec<Segment> {
    match highlighter.highlight_line(code, syntax_set()) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Segment {
                foreground: Some((style.foreground.r, style.foreground.g, style.foreground.b)),
                text: text.to_string(),
            })
            .collect(),
        Err(_) => vec![Segment::plain(code)],
    }
}

pub fn highlight_tool(
    name: &str,
    summary: &str,
    body: &str,
    dark: bool,
) -> Option<Vec<HighlightedLine>> {
    match name {
        "edit" | "write" => Some(highlight_body(syntax_for_path(summary), body, dark)),
        "bash" => Some(highlight_command(body, dark)),
        _ => None,
    }
}

pub fn normalise_lines(text: &str) -> Cow<'_, str> {
    if text.contains('\r') || text.ends_with('\n') {
        Cow::Owned(text.lines().collect::<Vec<_>>().join("\n"))
    } else {
        Cow::Borrowed(text)
    }
}

fn highlight_command(body: &str, dark: bool) -> Vec<HighlightedLine> {
    let bash = syntax_for_path("sh");
    let command = normalise_lines(body);
    let embedded = embedded_code(&command);
    let mut bash_highlighter = HighlightLines::new(bash, theme(dark));
    let mut embedded_highlighters: Vec<HighlightLines> = embedded
        .iter()
        .map(|code| HighlightLines::new(code.language.syntax(), theme(dark)))
        .collect();
    let mut line_start = 0;
    command
        .split('\n')
        .map(|line| {
            let line_end = line_start + line.len();
            let mut segments = Vec::new();
            let mut position = line_start;
            for (code, highlighter) in embedded.iter().zip(&mut embedded_highlighters) {
                let start = code.range.start.clamp(line_start, line_end);
                let end = code.range.end.clamp(line_start, line_end);
                if start >= end {
                    continue;
                }
                if position < start {
                    segments.extend(highlight_code(
                        &mut bash_highlighter,
                        &command[position..start],
                    ));
                }
                segments.extend(highlight_code(highlighter, &command[start..end]));
                position = end;
            }
            if position < line_end || line.is_empty() {
                segments.extend(highlight_code(
                    &mut bash_highlighter,
                    &command[position..line_end],
                ));
            }
            line_start = line_end + 1;
            HighlightedLine {
                change: Some(Change::Equal),
                segments,
            }
        })
        .collect()
}

fn highlight_body(syntax: &SyntaxReference, body: &str, dark: bool) -> Vec<HighlightedLine> {
    let mut old_highlighter = HighlightLines::new(syntax, theme(dark));
    let mut new_highlighter = HighlightLines::new(syntax, theme(dark));
    body.lines()
        .map(|line| {
            let mut characters = line.chars();
            let change = characters.next().and_then(change_for_sign);
            let Some(change) = change else {
                return HighlightedLine {
                    change: None,
                    segments: vec![Segment::plain(line)],
                };
            };
            let code = characters.as_str();
            let code_segments = match change {
                Change::Delete => highlight_code(&mut old_highlighter, code),
                Change::Insert => highlight_code(&mut new_highlighter, code),
                Change::Equal => {
                    highlight_code(&mut old_highlighter, code);
                    highlight_code(&mut new_highlighter, code)
                }
            };
            let mut segments = vec![Segment::plain(&line[..1])];
            segments.extend(code_segments);
            HighlightedLine {
                change: Some(change),
                segments,
            }
        })
        .collect()
}

/// Removes control characters other than newline and tab, such as escape
/// sequences that would clear the screen or set the clipboard, so text from
/// the model or a tool can be written to a terminal as plain text. Carriage
/// returns go too: a lone one could overwrite what is shown, and a terminal
/// already starts a new line at column one after a newline.
pub fn strip_controls(text: &str) -> Cow<'_, str> {
    let removed = |c: char| c.is_control() && c != '\n' && c != '\t';
    if text.contains(removed) {
        text.chars()
            .filter(|&c| !removed(c))
            .collect::<String>()
            .into()
    } else {
        text.into()
    }
}

#[cfg(test)]
mod tests;
