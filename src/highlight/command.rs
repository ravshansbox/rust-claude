use syntect::easy::HighlightLines;

use super::{
    Change, HighlightedLine, embedded::embedded_code, highlight_code, normalise_lines,
    syntax_for_path, theme,
};

pub(super) fn highlight_command(body: &str, dark: bool) -> Vec<HighlightedLine> {
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
