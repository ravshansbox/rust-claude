use std::{borrow::Cow, io::Cursor, sync::OnceLock};

use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

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

fn syntax_by_extension(extension: &str) -> &'static SyntaxReference {
    let syntax_set = syntax_set();
    syntax_set
        .find_syntax_by_extension(extension)
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

pub fn highlight_tool(
    name: &str,
    summary: &str,
    body: &str,
    dark: bool,
) -> Option<Vec<HighlightedLine>> {
    let syntax = match name {
        "edit" | "write" => syntax_for_path(summary),
        "bash" => return Some(highlight_command(body, dark)),
        _ => return None,
    };
    Some(highlight_body(syntax, body, dark))
}

#[derive(Clone, Copy)]
enum Language {
    Python,
    JavaScript,
    Ruby,
}

impl Language {
    fn of(program: &str) -> Option<Self> {
        let name = program.rsplit('/').next()?;
        let python_version = name.strip_prefix("python3.").is_some_and(|version| {
            !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
        });
        match name {
            "python" | "python3" => Some(Self::Python),
            "node" => Some(Self::JavaScript),
            "ruby" => Some(Self::Ruby),
            _ if python_version => Some(Self::Python),
            _ => None,
        }
    }

    fn syntax(self) -> &'static SyntaxReference {
        let extension = match self {
            Self::Python => "py",
            Self::JavaScript => "js",
            Self::Ruby => "rb",
        };
        syntax_by_extension(extension)
    }

    fn takes_code(self, flag: &str) -> bool {
        match self {
            Self::Python => flag == "-c",
            Self::JavaScript => matches!(flag, "-e" | "--eval" | "-p" | "--print"),
            Self::Ruby => flag == "-e",
        }
    }
}

struct Embedded {
    language: Language,
    range: std::ops::Range<usize>,
}

fn embedded_code(command: &str) -> Vec<Embedded> {
    let mut found = Vec::new();
    let mut position = 0;
    while let Some(rest) = command.get(position..).filter(|rest| !rest.is_empty()) {
        let word_length = rest.find(ends_word).unwrap_or(rest.len());
        if word_length == 0 {
            position += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let word_end = position + word_length;
        let embedded = Language::of(&rest[..word_length]).and_then(|language| {
            let range = code_after(command, word_end, language)?;
            Some(Embedded { language, range })
        });
        match embedded {
            Some(embedded) => {
                position = embedded.range.end.max(word_end);
                found.push(embedded);
            }
            None => position = word_end,
        }
    }
    found
}

fn ends_word(character: char) -> bool {
    character.is_whitespace() || ";|&()<>".contains(character)
}

fn code_after(command: &str, start: usize, language: Language) -> Option<std::ops::Range<usize>> {
    let line_end = command[start..]
        .find('\n')
        .map_or(command.len(), |offset| start + offset);
    let mut position = start;
    loop {
        let rest = command[position..line_end].trim_start();
        position = line_end - rest.len();
        if let Some(heredoc) = rest.strip_prefix("<<") {
            if heredoc.starts_with('<') {
                return None;
            }
            return heredoc_body(command, heredoc, line_end);
        }
        let token_length = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..token_length];
        if !token.starts_with('-') {
            return None;
        }
        position += token_length;
        if language.takes_code(token) {
            let argument = command[position..].trim_start_matches([' ', '\t']);
            let quote_start = command.len() - argument.len();
            let quote = argument
                .chars()
                .next()
                .filter(|character| matches!(character, '\'' | '"'))?;
            return Some(quoted_body(command, quote_start + 1, quote));
        }
    }
}

fn heredoc_body(command: &str, heredoc: &str, line_end: usize) -> Option<std::ops::Range<usize>> {
    let specification = heredoc.strip_prefix('-').unwrap_or(heredoc).trim_start();
    let delimiter: String = specification
        .chars()
        .take_while(|character| !ends_word(*character))
        .filter(|character| !matches!(character, '\'' | '"' | '\\'))
        .collect();
    if delimiter.is_empty() || line_end >= command.len() {
        return None;
    }
    let body_start = line_end + 1;
    let mut line_start = body_start;
    for line in command[body_start..].split('\n') {
        if line.trim() == delimiter {
            return Some(body_start..line_start.saturating_sub(1).max(body_start));
        }
        line_start += line.len() + 1;
    }
    Some(body_start..command.len())
}

fn quoted_body(command: &str, start: usize, quote: char) -> std::ops::Range<usize> {
    let mut escaped = false;
    for (offset, character) in command[start..].char_indices() {
        if escaped {
            escaped = false;
        } else if quote == '"' && character == '\\' {
            escaped = true;
        } else if character == quote {
            return start..start + offset;
        }
    }
    start..command.len()
}

pub fn normalise_lines(text: &str) -> Cow<'_, str> {
    if text.contains('\r') || text.ends_with('\n') {
        let mut normalised = String::with_capacity(text.len());
        for (index, line) in text.lines().enumerate() {
            if index > 0 {
                normalised.push('\n');
            }
            normalised.push_str(line);
        }
        Cow::Owned(normalised)
    } else {
        Cow::Borrowed(text)
    }
}

fn highlight_command(body: &str, dark: bool) -> Vec<HighlightedLine> {
    let bash = syntax_by_extension("sh");
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

/// Removes control characters other than newline and tab, such as escape
/// sequences that would clear the screen or set the clipboard, so text from
/// the model or a tool can be written to a terminal as plain text. Carriage
/// returns go too: a lone one could overwrite what is shown, and a terminal
/// already starts a new line at column one after a newline.
pub fn strip_controls(text: &str) -> std::borrow::Cow<'_, str> {
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
mod tests {
    use super::*;

    #[test]
    fn marks_changes_and_keeps_text() {
        let lines = highlight_tool(
            "edit",
            "main.rs",
            "-let a = 1;\n+let a = 2;\n fn b() {}\n… 3 more lines",
            true,
        )
        .unwrap();
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

    fn bash(command: &str) -> Vec<HighlightedLine> {
        highlight_tool("bash", "", command, true).unwrap()
    }

    fn colour_of(lines: &[HighlightedLine], word: &str) -> Option<Colour> {
        lines
            .iter()
            .flat_map(|line| &line.segments)
            .find(|segment| segment.text == word)
            .and_then(|segment| segment.foreground)
    }

    fn keyword_colour(path: &str, code: &str, word: &str) -> Colour {
        let lines = highlight_tool("write", path, &format!("+{code}"), true).unwrap();
        colour_of(&lines, word).unwrap()
    }

    fn texts(lines: &[HighlightedLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.segments
                    .iter()
                    .map(|segment| segment.text.as_str())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn highlights_python_heredocs_in_bash() {
        let command = "python3 - <<'EOF'\nimport os\ndef f(x):\n  return 1\nEOF\necho done";
        let lines = bash(command);
        assert_eq!(texts(&lines), command.lines().collect::<Vec<_>>());
        let python =
            highlight_tool("write", "f.py", " import os\n def f(x):\n   return 1", true).unwrap();
        for (line, python) in lines[1..4].iter().zip(&python) {
            assert_eq!(line.segments[..], python.segments[1..]);
        }
        assert_eq!(lines[4], bash("python3 - <<'EOF'\nEOF")[1]);
    }

    #[test]
    fn highlights_python_after_dash_c_in_bash() {
        let keyword = keyword_colour("x.py", "import sys", "import");
        let single = bash("python -u -c 'import sys; print(1)' | wc -c");
        assert_eq!(
            texts(&single),
            ["python -u -c 'import sys; print(1)' | wc -c"]
        );
        assert_eq!(colour_of(&single, "import"), Some(keyword));
        assert_eq!(colour_of(&single, "wc"), colour_of(&bash("wc -c"), "wc"));
        let multiple = bash("python3 -c \"\nimport sys\nprint(\\\"a\\\")\n\"\necho done");
        assert_eq!(colour_of(&multiple, "import"), Some(keyword));
        assert_eq!(
            colour_of(&multiple, "echo"),
            colour_of(&bash("echo"), "echo")
        );
    }

    #[test]
    fn highlights_node_and_ruby_in_bash() {
        let javascript = bash("node -e 'const a = 1; console.log(a)'");
        assert_eq!(
            colour_of(&javascript, "const"),
            Some(keyword_colour("x.js", "const a = 1;", "const"))
        );
        let ruby = bash("ruby <<RUBY\ndef f\nend\nRUBY");
        assert_eq!(
            colour_of(&ruby, "def"),
            Some(keyword_colour("x.rb", "def f", "def"))
        );
    }

    #[test]
    fn leaves_input_to_scripts_as_bash() {
        let lines = bash("python3 script.py <<EOF\nimport os\nEOF");
        assert_eq!(colour_of(&lines, "import"), None);
    }
}
