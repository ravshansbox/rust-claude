use crate::agent::{Agent, AgentEvent};
use anyhow::Result;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::Stylize,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::mpsc;

#[derive(Clone, Copy)]
enum Role {
    User,
    Assistant,
    Tool,
    Event,
}

struct ChatMessage {
    role: Role,
    text: String,
}

struct App {
    input: String,
    messages: Vec<ChatMessage>,
    status: String,
    usage: String,
    scroll_from_bottom: u16,
    max_scroll: u16,
    page_size: u16,
    busy: bool,
}

impl App {
    fn new(model: &str) -> Self {
        Self {
            input: String::new(),
            messages: vec![ChatMessage {
                role: Role::Event,
                text: "Ask me to inspect, explain, or edit this project.".into(),
            }],
            status: format!("ready · {model}"),
            usage: String::new(),
            scroll_from_bottom: 0,
            max_scroll: 0,
            page_size: 1,
            busy: false,
        }
    }

    fn push(&mut self, role: Role, text: impl Into<String>) {
        self.messages.push(ChatMessage {
            role,
            text: text.into(),
        });
    }

    fn scroll_up(&mut self, amount: u16) {
        self.scroll_from_bottom = self
            .scroll_from_bottom
            .saturating_add(amount)
            .min(self.max_scroll);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(amount);
    }

    fn scroll_to_top(&mut self) {
        self.scroll_from_bottom = self.max_scroll;
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }
}

pub async fn run(agent: Agent) -> Result<()> {
    let mut terminal = ratatui::init();
    if let Err(error) = execute!(std::io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(error.into());
    }

    let result = run_loop(&mut terminal, agent).await;
    let mouse_result = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    mouse_result?;
    result
}

enum UiEvent {
    Agent(AgentEvent),
    Done(Result<()>),
    Cancelled,
}

async fn agent_task(
    mut agent: Agent,
    mut prompts: mpsc::UnboundedReceiver<String>,
    mut cancel: mpsc::UnboundedReceiver<()>,
    events: mpsc::UnboundedSender<UiEvent>,
) {
    while let Some(prompt) = prompts.recv().await {
        while cancel.try_recv().is_ok() {}

        let checkpoint = agent.history_len();
        let cancelled = {
            let run = agent.prompt(&prompt, |event| {
                let _ = events.send(UiEvent::Agent(event));
            });
            tokio::select! {
                result = run => {
                    let _ = events.send(UiEvent::Done(result));
                    false
                }
                _ = cancel.recv() => true,
            }
        };
        if cancelled {
            agent.rollback(checkpoint);
            let _ = events.send(UiEvent::Cancelled);
        }
    }
}

async fn run_loop(terminal: &mut DefaultTerminal, agent: Agent) -> Result<()> {
    let mut app = App::new(&agent.model);
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, prompt_rx, cancel_rx, event_tx));
    let mut terminal_events = EventStream::new();

    let result = loop {
        terminal.draw(|frame| draw(frame, &mut app))?;

        tokio::select! {
            event = terminal_events.next() => {
                let event = match event {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => break Err(error.into()),
                    None => break Ok(()),
                };
                let quit = handle_input(event, &mut app, |action| match action {
                    Action::Submit(prompt) => {
                        let _ = prompt_tx.send(prompt);
                    }
                    Action::Cancel => {
                        let _ = cancel_tx.send(());
                    }
                });
                if quit {
                    break Ok(());
                }
            }
            Some(event) = event_rx.recv() => {
                let mut next = Some(event);
                while let Some(event) = next {
                    handle_agent_event(event, &mut app);
                    next = event_rx.try_recv().ok();
                }
            }
        }
    };
    worker.abort();
    result
}

enum Action {
    Submit(String),
    Cancel,
}

fn handle_input(event: Event, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    if let Event::Mouse(mouse) = event {
        match mouse.kind {
            MouseEventKind::ScrollUp => app.scroll_up(3),
            MouseEventKind::ScrollDown => app.scroll_down(3),
            _ => {}
        }
        return false;
    }
    let Event::Key(key) = event else { return false };
    if key.kind != KeyEventKind::Press {
        return false;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
        (KeyCode::Esc, _) if app.busy => {
            app.status = "cancelling".into();
            act(Action::Cancel);
        }
        (KeyCode::Esc, _) => return true,
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => return true,
        (KeyCode::Up, _) => app.scroll_up(1),
        (KeyCode::Down, _) => app.scroll_down(1),
        (KeyCode::PageUp, _) => app.scroll_up(app.page_size),
        (KeyCode::PageDown, _) => app.scroll_down(app.page_size),
        (KeyCode::Home, _) => app.scroll_to_top(),
        (KeyCode::End, _) => app.scroll_to_bottom(),
        (KeyCode::Enter, _) if !app.busy && !app.input.trim().is_empty() => {
            app.scroll_to_bottom();
            let prompt = std::mem::take(&mut app.input);
            app.push(Role::User, prompt.clone());
            app.status = "thinking".into();
            app.busy = true;
            act(Action::Submit(prompt));
        }
        (KeyCode::Backspace, _) => {
            app.input.pop();
        }
        (KeyCode::Char(character), _) => app.input.push(character),
        _ => {}
    }
    false
}

fn handle_agent_event(event: UiEvent, app: &mut App) {
    match event {
        UiEvent::Agent(AgentEvent::Text(text)) => match app.messages.last_mut() {
            Some(last) if matches!(last.role, Role::Assistant) => last.text.push_str(&text),
            _ => app.push(Role::Assistant, text),
        },
        UiEvent::Agent(AgentEvent::ToolCall(name)) => app.status = format!("calling {name}"),
        UiEvent::Agent(AgentEvent::ToolStart { name, summary }) => {
            app.status = format!("running {name}");
            app.push(Role::Tool, format!("{name} {summary}"));
        }
        UiEvent::Agent(AgentEvent::ToolDone { name, error }) => {
            app.status = format!("used {name}");
            if let Some(error) = error {
                app.push(Role::Event, format!("{name} failed: {error}"));
            }
        }
        UiEvent::Agent(AgentEvent::Usage { input, output }) => {
            app.usage = format!(
                "{} in · {} out",
                format_tokens(input),
                format_tokens(output)
            );
        }
        UiEvent::Done(result) => {
            if let Err(error) = result {
                app.push(Role::Event, format!("error: {error}"));
            }
            app.status = "ready".into();
            app.busy = false;
        }
        UiEvent::Cancelled => {
            app.push(Role::Event, "cancelled");
            app.status = "ready".into();
            app.busy = false;
        }
    }
}

fn format_tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..999_950 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let [chat, input, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    let mut lines = Vec::new();
    for message in &app.messages {
        let label = match message.role {
            Role::User => "> ".cyan().bold(),
            Role::Assistant => {
                lines.extend(tui_markdown::from_str(&message.text).lines);
                lines.push(Line::default());
                continue;
            }
            Role::Tool => "⏺ ".yellow(),
            Role::Event => "· ".dark_gray(),
        };
        for (index, text) in message.text.lines().enumerate() {
            let prefix = if index == 0 {
                label.clone()
            } else {
                "  ".into()
            };
            lines.push(Line::from(vec![prefix, Span::raw(text)]));
        }
        lines.push(Line::default());
    }

    let transcript = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(" rust-claude ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    let viewport_height = chat.height.saturating_sub(2);
    let wrapped_line_count = transcript.line_count(chat.width);
    app.max_scroll = wrapped_line_count
        .saturating_sub(viewport_height as usize)
        .min(u16::MAX as usize) as u16;
    app.page_size = viewport_height.max(1);
    app.scroll_from_bottom = app.scroll_from_bottom.min(app.max_scroll);
    let scroll = app.max_scroll.saturating_sub(app.scroll_from_bottom);
    frame.render_widget(transcript.scroll((scroll, 0)), chat);

    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .block(Block::default().title(" Prompt ").borders(Borders::ALL)),
        input,
    );
    frame.set_cursor_position((input.x + app.input.chars().count() as u16 + 1, input.y + 1));

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!(" {}", app.status)),
            Span::raw(if app.usage.is_empty() {
                "".into()
            } else {
                format!(" · {}", app.usage)
            }),
            if app.busy {
                "   ↑/↓ scroll · PgUp/PgDn · Esc cancel".dark_gray()
            } else {
                "   ↑/↓ scroll · PgUp/PgDn · Esc quit".dark_gray()
            },
        ])),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::format_tokens;

    #[test]
    fn formats_tokens() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(12_345), "12.3k");
        assert_eq!(format_tokens(999_950), "1.0M");
        assert_eq!(format_tokens(1_234_567), "1.2M");
    }
}
