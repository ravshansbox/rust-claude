use super::{draw_chat, saturating_u16};
use crate::tui::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Stylize,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

/// Draws the agent on `task` in place of the main session, with a tab for
/// each running agent, its description and no prompt.
pub(super) fn draw_agent(frame: &mut Frame, app: &mut App, task: usize) {
    let width = frame.area().width;
    let tabs = tab_line(app, task);
    let description = app
        .task_tabs()
        .into_iter()
        .find_map(|(shown, description)| (shown == task).then(|| description.to_string()))
        .unwrap_or_default();
    let description = Paragraph::new(Line::raw(description)).wrap(Wrap { trim: false });
    let description_height = saturating_u16(description.line_count(width));
    let mut footer = app.task_stats(task);
    footer.extend(["←/→ switch agent".into(), "Esc back".into()]);
    let footer = Paragraph::new(Line::raw(footer.join(" · "))).wrap(Wrap { trim: false });
    let footer_height = saturating_u16(footer.line_count(width));
    let [tabs_area, description_area, chat, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(description_height),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(frame.area());
    frame.render_widget(Paragraph::new(tabs), tabs_area);
    frame.render_widget(description, description_area);
    let spinner_frame = app.spinner_frame;
    if let Some(view) = app.task_view(task) {
        view.spinner_frame = spinner_frame;
        draw_chat(frame, view, chat, 0);
    }
    frame.render_widget(footer, footer_area);
}

fn tab_line(app: &App, shown: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for (task, _) in app.task_tabs() {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        let label = format!(" {} ", task + 1);
        spans.push(if task == shown {
            label.reversed()
        } else {
            label.dark_gray()
        });
    }
    Line::from(spans)
}
