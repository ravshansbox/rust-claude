use std::sync::OnceLock;

use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

pub type Colour = (u8, u8, u8);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Delete,
    Insert,
    Equal,
}

#[derive(Debug, PartialEq)]
pub struct Segment {
    pub foreground: Option<Colour>,
    pub text: String,
}

#[derive(Debug, PartialEq)]
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

fn theme(dark: bool) -> &'static Theme {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);
    let name = if dark {
        "base16-ocean.dark"
    } else {
        "base16-ocean.light"
    };
    &themes.themes[name]
}

fn syntax_for_path(path: &str) -> &'static SyntaxReference {
    let syntax_set = syntax_set();
    let path = std::path::Path::new(path);
    path.extension()
        .and_then(|extension| syntax_set.find_syntax_by_extension(extension.to_str()?))
        .or_else(|| {
            path.file_name()
                .and_then(|name| syntax_set.find_syntax_by_extension(name.to_str()?))
        })
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
        Err(_) => vec![Segment {
            foreground: None,
            text: code.to_string(),
        }],
    }
}

pub fn highlight_body(path: &str, body: &str, dark: bool) -> Vec<HighlightedLine> {
    let syntax = syntax_for_path(path);
    let mut old_highlighter = HighlightLines::new(syntax, theme(dark));
    let mut new_highlighter = HighlightLines::new(syntax, theme(dark));
    body.lines()
        .map(|line| {
            let mut characters = line.chars();
            let change = characters.next().and_then(change_for_sign);
            let Some(change) = change else {
                return HighlightedLine {
                    change: None,
                    segments: vec![Segment {
                        foreground: None,
                        text: line.to_string(),
                    }],
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
            let mut segments = vec![Segment {
                foreground: None,
                text: line[..1].to_string(),
            }];
            segments.extend(code_segments);
            HighlightedLine {
                change: Some(change),
                segments,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_changes_and_keeps_text() {
        let lines = highlight_body(
            "main.rs",
            "-let a = 1;\n+let a = 2;\n fn b() {}\n… 3 more lines",
            true,
        );
        let changes: Vec<Option<Change>> = lines.iter().map(|line| line.change).collect();
        assert_eq!(
            changes,
            vec![
                Some(Change::Delete),
                Some(Change::Insert),
                Some(Change::Equal),
                None
            ]
        );
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                line.segments
                    .iter()
                    .map(|segment| segment.text.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(
            texts,
            vec!["-let a = 1;", "+let a = 2;", " fn b() {}", "… 3 more lines"]
        );
        assert!(lines[0].segments.len() > 2);
        assert_eq!(lines[3].segments[0].foreground, None);
    }
}
