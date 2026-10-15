use super::reversed_if;
use crate::tui::{HistorySearch, Picker};
use ratatui::{
    style::Stylize,
    text::{Line, Span},
    widgets::Paragraph,
};
use std::borrow::Cow;

pub(super) fn picker_view(picker: &Picker, height: u16) -> Paragraph<'_> {
    let mut lines = vec![Line::from(
        format!("{} (↑↓ select, Enter confirm, Esc cancel)", picker.title).bold(),
    )];
    let visible = usize::from(height).saturating_sub(1).max(1);
    let first = (picker.selected + 1).saturating_sub(visible);
    for (index, (_, text)) in picker.items.iter().enumerate().skip(first).take(visible) {
        lines.push(Line::from(reversed_if(
            Span::from(text.as_str()),
            index == picker.selected,
        )));
    }
    Paragraph::new(lines)
}

pub(super) fn history_view(search: &HistorySearch, height: u16) -> Paragraph<'_> {
    let mut lines = vec![
        Line::from(vec![
            Span::from("Prompt history ").bold(),
            reversed_if(Span::from(" Folder "), !search.all),
            Span::raw(" "),
            reversed_if(Span::from(" All "), search.all),
            Span::from(" (←→ tab, ↑↓ select, Enter edit, Esc cancel)").bold(),
        ]),
        Line::raw(format!("search: {}", search.query)),
    ];
    let matches = search.matches();
    let visible = usize::from(height).saturating_sub(2).max(1);
    let first = (search.selected + 1).saturating_sub(visible);
    for (index, (prompt, folder)) in matches.into_iter().enumerate().skip(first).take(visible) {
        let mut prompt_lines = prompt.lines();
        let first_line = prompt_lines.next().unwrap_or_default();
        let text: Cow<str> = if prompt_lines.next().is_some() {
            format!("{first_line} …").into()
        } else {
            first_line.into()
        };
        let mut spans = vec![reversed_if(Span::from(text), index == search.selected)];
        if let Some(folder) = folder {
            let name = std::path::Path::new(folder)
                .file_name()
                .map_or(Cow::Borrowed(folder), |name| name.to_string_lossy());
            spans.push(Span::from(format!("  {name}")).dark_gray());
        }
        lines.push(Line::from(spans));
    }
    Paragraph::new(lines)
}
