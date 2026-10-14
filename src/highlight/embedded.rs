use std::ops::Range;

use syntect::parsing::SyntaxReference;

use super::syntax_for_path;

#[derive(Clone, Copy)]
pub(super) enum Language {
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

    pub(super) fn syntax(self) -> &'static SyntaxReference {
        let extension = match self {
            Self::Python => "py",
            Self::JavaScript => "js",
            Self::Ruby => "rb",
        };
        syntax_for_path(extension)
    }

    fn takes_code(self, flag: &str) -> bool {
        match self {
            Self::Python => flag == "-c",
            Self::JavaScript => matches!(flag, "-e" | "--eval" | "-p" | "--print"),
            Self::Ruby => flag == "-e",
        }
    }
}

pub(super) struct Embedded {
    pub(super) language: Language,
    pub(super) range: Range<usize>,
}

pub(super) fn embedded_code(command: &str) -> Vec<Embedded> {
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

fn code_after(command: &str, start: usize, language: Language) -> Option<Range<usize>> {
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

fn heredoc_body(command: &str, heredoc: &str, line_end: usize) -> Option<Range<usize>> {
    let strips_tabs = heredoc.starts_with('-');
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
        let text = if strips_tabs {
            line.trim_start_matches('\t')
        } else {
            line
        };
        if text == delimiter {
            return Some(body_start..line_start.saturating_sub(1).max(body_start));
        }
        line_start += line.len() + 1;
    }
    Some(body_start..command.len())
}

fn quoted_body(command: &str, start: usize, quote: char) -> Range<usize> {
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
