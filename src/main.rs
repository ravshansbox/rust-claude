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
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let print_prompt = match arguments.as_slice() {
        [] => None,
        [flag, prompt] if flag == "-p" || flag == "--print" => Some(prompt.clone()),
        _ => bail!("usage: rust-claude [-p|--print <prompt>]"),
    };

    let http = reqwest::Client::new();
    let credentials = auth::Credentials::load_or_login(&http).await?;
    let settings = settings::Settings::load();
    let model = std::env::var("RUST_CLAUDE_MODEL")
        .ok()
        .or(settings.model)
        .unwrap_or_else(|| "claude-opus-5-5".into());
    let mut agent = agent::Agent::new(http, credentials, model)?;
    if let Some(level) = agent::THINKING_LEVELS
        .iter()
        .find(|level| Some(**level) == settings.thinking_level.as_deref())
    {
        agent.thinking_level = level;
    }

    let Some(prompt) = print_prompt else {
        return tui::run(agent).await;
    };
    let mut stdout = std::io::stdout();
    agent
        .prompt(&prompt, |event| match event {
            agent::AgentEvent::Text(text) => {
                let _ = write!(stdout, "{text}");
                let _ = stdout.flush();
            }
            agent::AgentEvent::Notice(text) => eprintln!("\n{text}"),
            _ => {}
        })
        .await?;
    writeln!(stdout)?;
    Ok(())
}
