use syntect::{easy::HighlightLines, parsing::SyntaxReference};

use super::{Change, HighlightedLine, Segment, highlight_code, theme};

fn change_for_sign(sign: char) -> Option<Change> {
    match sign {
        '-' => Some(Change::Delete),
        '+' => Some(Change::Insert),
        ' ' => Some(Change::Equal),
        _ => None,
    }
}

pub(super) fn highlight_body(
    syntax: &SyntaxReference,
    body: &str,
    dark: bool,
) -> Vec<HighlightedLine> {
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
