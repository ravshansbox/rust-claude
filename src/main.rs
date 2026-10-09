mod agent;
mod auth;
mod models;
mod session;
mod settings;
mod tools;
mod tui;

use std::io::Write;

use anyhow::{Result, bail};

#[tokio::main]
async fn main() -> Result<()> {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    let hide_tools = match arguments
        .iter()
        .position(|argument| argument == "--hide-tools")
    {
        Some(index) => {
            arguments.remove(index);
            true
        }
        None => false,
    };
    let print_prompt = match arguments.as_slice() {
        [] if !hide_tools => None,
        [flag, prompt] if flag == "-p" || flag == "--print" => Some(prompt.clone()),
        _ => bail!("usage: rust-claude [-p|--print <prompt> [--hide-tools]]"),
    };

    let http = reqwest::Client::new();
    let credentials = auth::Credentials::load_or_login(&http).await?;
    let settings = settings::Settings::load();
    let model = std::env::var("RUST_CLAUDE_MODEL")
        .ok()
        .or(settings.model)
        .unwrap_or_else(|| "claude-opus-5-5".into());
    let thinking_level = std::env::var("RUST_CLAUDE_THINKING")
        .ok()
        .or(settings.thinking_level);
    let mut agent = agent::Agent::new(http, credentials, model)?;
    if let Some(level) = agent::THINKING_LEVELS
        .iter()
        .find(|level| Some(**level) == thinking_level.as_deref())
    {
        agent.thinking_level = level;
    }

    let Some(prompt) = print_prompt else {
        return tui::run(agent).await;
    };
    let mut stdout = std::io::stdout();
    let mut printed = false;
    let mut separate = false;
    let mut line_open = false;
    agent
        .prompt(&prompt, |event| match event {
            agent::AgentEvent::Text(text) => {
                if separate {
                    let _ = write!(stdout, "\n\n");
                    separate = false;
                }
                let _ = write!(stdout, "{text}");
                let _ = stdout.flush();
                printed = true;
                line_open = !text.ends_with('\n');
            }
            agent::AgentEvent::ToolStart { name, summary } => {
                separate = printed;
                if !hide_tools {
                    if line_open {
                        eprintln!();
                        line_open = false;
                    }
                    eprintln!("{name} {summary}");
                }
            }
            agent::AgentEvent::ToolDone {
                name,
                error: Some(error),
            } if !hide_tools => {
                if line_open {
                    eprintln!();
                    line_open = false;
                }
                eprintln!("{name} failed: {error}");
            }
            agent::AgentEvent::Notice(text) => eprintln!("\n{text}"),
            _ => {}
        })
        .await?;
    writeln!(stdout)?;
    Ok(())
}
