mod agent;
mod auth;
mod models;
mod session;
mod settings;
mod tools;
mod tui;

use std::io::Write;

use anyhow::{Result, bail};

const USAGE: &str = "usage: rust-claude [-h|--help] [-p|--print <prompt> [--hide-tools]]";

const HELP: &str = "A small coding agent for the terminal.

usage: rust-claude [-h|--help] [-p|--print <prompt> [--hide-tools]]

Options:
  -p, --print <prompt>  Run one prompt and print the answer
      --hide-tools      Hide tool calls in print mode
  -h, --help            Show this help

Environment variables:
  RUST_CLAUDE_MODEL     Model to use
  RUST_CLAUDE_THINKING  Thinking level: low, medium, high, xhigh, max";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Interactive,
    Print { prompt: String, hide_tools: bool },
}

fn parse_arguments(mut arguments: Vec<String>) -> Result<Command> {
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
    match arguments.as_slice() {
        [flag] if !hide_tools && (flag == "-h" || flag == "--help") => Ok(Command::Help),
        [] if !hide_tools => Ok(Command::Interactive),
        [flag, prompt] if flag == "-p" || flag == "--print" => Ok(Command::Print {
            prompt: prompt.clone(),
            hide_tools,
        }),
        _ => bail!(USAGE),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (print_prompt, hide_tools) = match parse_arguments(std::env::args().skip(1).collect())? {
        Command::Help => {
            println!("{HELP}");
            return Ok(());
        }
        Command::Interactive => (None, false),
        Command::Print { prompt, hide_tools } => (Some(prompt), hide_tools),
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

#[cfg(test)]
mod tests {
    use super::{Command, parse_arguments};

    fn parse(arguments: &[&str]) -> Option<Command> {
        parse_arguments(
            arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect(),
        )
        .ok()
    }

    #[test]
    fn parses_help() {
        assert_eq!(parse(&["--help"]), Some(Command::Help));
        assert_eq!(parse(&["-h"]), Some(Command::Help));
    }

    #[test]
    fn rejects_help_with_other_arguments() {
        assert_eq!(parse(&["--help", "--hide-tools"]), None);
        assert_eq!(parse(&["-h", "extra"]), None);
    }

    #[test]
    fn parses_print_and_interactive() {
        assert_eq!(parse(&[]), Some(Command::Interactive));
        assert_eq!(
            parse(&["--print", "hello", "--hide-tools"]),
            Some(Command::Print {
                prompt: "hello".into(),
                hide_tools: true,
            })
        );
    }
}
