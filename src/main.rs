mod agent;
mod auth;
mod clipboard;
mod config;
mod highlight;
mod history;
mod images;
mod mcp;
mod models;
mod session;
mod settings;
mod skills;
mod tools;
mod tui;

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: rust-claude [-h|--help] [-c|--continue] [--config-dir <path>] [--model <id>] [--thinking <level>] [-p|--print <prompt> [--image <path>]...]";

const HELP: &str = "A small coding agent for the terminal.

usage: rust-claude [-h|--help] [-c|--continue] [--config-dir <path>] [--model <id>] [--thinking <level>] [-p|--print <prompt> [--image <path>]...]

Options:
  -c, --continue           Continue the latest session in the current folder
      --config-dir <path>  Folder for sign-in, settings, sessions, history, skills and MCP config. Default: ~/.rust-claude
      --model <id>         Model to use
      --thinking <level>   Thinking level: low, medium, high, xhigh, max
  -p, --print <prompt>     Run one prompt and print the answer
      --image <path>       Send an image with the prompt in print mode. Repeat for more images
  -h, --help               Show this help";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Interactive,
    Print { prompt: String, images: Vec<String> },
}

#[derive(Debug, Default, PartialEq)]
struct Options {
    model: Option<String>,
    thinking_level: Option<String>,
    continue_session: bool,
    config_dir: Option<String>,
}

fn take_flag(arguments: &mut Vec<String>, flags: &[&str]) -> bool {
    match arguments
        .iter()
        .position(|argument| flags.contains(&argument.as_str()))
    {
        Some(index) => {
            arguments.remove(index);
            true
        }
        None => false,
    }
}

fn take_value(arguments: &mut Vec<String>, flag: &str) -> Result<Option<String>> {
    let Some(index) = arguments.iter().position(|argument| argument == flag) else {
        return Ok(None);
    };
    if index + 1 >= arguments.len() {
        bail!(USAGE);
    }
    let value = arguments.remove(index + 1);
    arguments.remove(index);
    Ok(Some(value))
}

fn parse_arguments(mut arguments: Vec<String>) -> Result<(Command, Options)> {
    let mut images = Vec::new();
    while let Some(image) = take_value(&mut arguments, "--image")? {
        images.push(image);
    }
    let options = Options {
        model: take_value(&mut arguments, "--model")?,
        thinking_level: take_value(&mut arguments, "--thinking")?,
        continue_session: take_flag(&mut arguments, &["-c", "--continue"]),
        config_dir: take_value(&mut arguments, "--config-dir")?,
    };
    let print_options = !images.is_empty();
    let command = match arguments.as_slice() {
        [flag]
            if !print_options
                && options == Options::default()
                && (flag == "-h" || flag == "--help") =>
        {
            Command::Help
        }
        [] if !print_options => Command::Interactive,
        [flag, prompt] if flag == "-p" || flag == "--print" => Command::Print {
            prompt: prompt.clone(),
            images,
        },
        _ => bail!(USAGE),
    };
    Ok((command, options))
}

#[tokio::main]
async fn main() -> Result<()> {
    let (command, options) = parse_arguments(std::env::args().skip(1).collect())?;
    if let Some(config_dir) = &options.config_dir {
        config::set_dir(config_dir.into());
    }
    let (print_prompt, image_paths) = match command {
        Command::Help => {
            println!("{HELP}");
            return Ok(());
        }
        Command::Interactive => (None, Vec::new()),
        Command::Print { prompt, images } => (Some(prompt), images),
    };
    let images = image_paths
        .iter()
        .map(|path| {
            std::fs::read(path)
                .map_err(anyhow::Error::from)
                .and_then(images::prepare)
                .with_context(|| format!("failed to load image {path}"))
        })
        .collect::<Result<Vec<_>>>()?;

    let http = agent::http_client()?;
    let credentials = auth::Credentials::load_or_login(&http).await?;
    let settings = settings::Settings::load();
    let model = options
        .model
        .or(settings.model)
        .unwrap_or_else(|| "claude-opus-5-5".into());
    let thinking_level = options.thinking_level.or(settings.thinking_level);
    let mut agent = agent::Agent::new(http, credentials, model)?;
    if options.continue_session {
        let id = session::Session::latest_in_current_folder()?
            .context("no session to continue in this folder")?;
        agent.resume(&id)?;
    }
    if let Some(name) = thinking_level {
        match agent::THINKING_LEVELS.iter().find(|level| **level == name) {
            Some(level) => agent.thinking_level = level,
            None => eprintln!(
                "unknown thinking level: {name} (options: {}), using {}",
                agent::THINKING_LEVELS.join(", "),
                agent::DEFAULT_THINKING_LEVEL
            ),
        }
    }

    let Some(prompt) = print_prompt else {
        return tui::run(agent).await;
    };
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    tokio::select! {
        mcp = mcp::Mcp::load() => agent.mcp = mcp,
        _ = &mut interrupt => return interrupted(agent),
    }
    for diagnostic in &agent.mcp.diagnostics {
        eprintln!("{diagnostic}");
    }
    for program in &agent.missing_programs {
        eprintln!("{program} not found on PATH");
    }
    let mut stdout = std::io::stdout();
    let mut printed = false;
    let mut separate = false;
    let mut line_open = false;
    let colour = std::io::stderr().is_terminal();
    let dark = colour && tui::dark_theme();
    let mut reads = tools::ReadGroup::default();
    let checkpoint = agent.history_len();
    let run = agent.prompt(&prompt, &images, |event| match event {
        agent::AgentEvent::Text(text) => {
            flush_reads(&mut reads);
            if separate {
                let _ = write!(stdout, "\n\n");
                separate = false;
            }
            let _ = write!(stdout, "{text}");
            let _ = stdout.flush();
            printed = true;
            line_open = !text.ends_with('\n');
        }
        agent::AgentEvent::ToolStart {
            name,
            summary,
            diff,
        } => {
            separate = printed;
            if line_open {
                eprintln!();
                line_open = false;
            }
            if name == "read" && diff.is_none() {
                reads.add(summary);
                return;
            }
            flush_reads(&mut reads);
            eprintln!("{name} {summary}");
            if let Some(diff) = diff {
                if colour {
                    for line in highlight::highlight_body(&summary, &diff, dark) {
                        eprintln!("{}", highlight::ansi_line(&line, dark));
                    }
                } else {
                    eprintln!("{diff}");
                }
            }
        }
        agent::AgentEvent::ToolDone {
            name,
            error: Some(error),
            ..
        } => {
            flush_reads(&mut reads);
            if line_open {
                eprintln!();
                line_open = false;
            }
            eprintln!("{name} failed: {error}");
        }
        agent::AgentEvent::ToolDone {
            name,
            note: Some(note),
            ..
        } => {
            flush_reads(&mut reads);
            eprintln!("{name}: {note}");
        }
        agent::AgentEvent::Notice(text) => {
            flush_reads(&mut reads);
            eprintln!("\n{text}");
        }
        _ => {}
    });
    let result = tokio::select! {
        result = run => Some(result),
        _ = &mut interrupt => None,
    };
    flush_reads(&mut reads);
    let Some(result) = result else {
        let saved = agent.cancel(checkpoint);
        if let Err(error) = saved {
            eprintln!("failed to save session: {error}");
        }
        return interrupted(agent);
    };
    result?;
    writeln!(stdout)?;
    Ok(())
}

fn interrupted(agent: agent::Agent) -> Result<()> {
    drop(agent);
    eprintln!("\ncancelled");
    std::process::exit(130);
}

fn flush_reads(reads: &mut tools::ReadGroup) {
    if !reads.is_empty() {
        eprintln!("read {}", reads.summary());
        reads.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, Options, parse_arguments};

    fn parse_with_options(arguments: &[&str]) -> Option<(Command, Options)> {
        parse_arguments(
            arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect(),
        )
        .ok()
    }

    fn parse(arguments: &[&str]) -> Option<Command> {
        parse_with_options(arguments).map(|(command, _)| command)
    }

    #[test]
    fn parses_help() {
        assert_eq!(parse(&["--help"]), Some(Command::Help));
        assert_eq!(parse(&["-h"]), Some(Command::Help));
    }

    #[test]
    fn rejects_help_with_other_arguments() {
        assert_eq!(parse(&["-h", "extra"]), None);
        assert_eq!(parse(&["--image", "a.png"]), None);
        assert_eq!(parse(&["-p", "hello", "--image"]), None);
        assert_eq!(parse(&["--help", "--model", "x"]), None);
        assert_eq!(parse(&["--model"]), None);
    }

    #[test]
    fn parses_print_and_interactive() {
        assert_eq!(parse(&[]), Some(Command::Interactive));
        assert_eq!(
            parse(&["--image", "a.png", "-p", "hello", "--image", "b.png"]),
            Some(Command::Print {
                prompt: "hello".into(),
                images: vec!["a.png".into(), "b.png".into()],
            })
        );
    }

    #[test]
    fn parses_continue_option() {
        for flag in ["-c", "--continue"] {
            assert_eq!(
                parse_with_options(&[flag]),
                Some((
                    Command::Interactive,
                    Options {
                        continue_session: true,
                        ..Options::default()
                    }
                ))
            );
        }
        assert_eq!(
            parse_with_options(&["-c", "-p", "hello"]).map(|(_, options)| options.continue_session),
            Some(true)
        );
        assert_eq!(parse(&["-c", "--help"]), None);
    }

    #[test]
    fn parses_config_dir_option() {
        assert_eq!(
            parse_with_options(&["--config-dir", "/tmp/rc", "-p", "hello"])
                .map(|(_, options)| options.config_dir),
            Some(Some("/tmp/rc".into()))
        );
        assert_eq!(parse(&["--config-dir"]), None);
        assert_eq!(parse(&["--config-dir", "/tmp/rc", "--help"]), None);
    }

    #[test]
    fn parses_model_and_thinking_options() {
        assert_eq!(
            parse_with_options(&["--model", "claude-x", "--thinking", "high"]),
            Some((
                Command::Interactive,
                Options {
                    model: Some("claude-x".into()),
                    thinking_level: Some("high".into()),
                    continue_session: false,
                    config_dir: None,
                }
            ))
        );
        assert_eq!(
            parse_with_options(&["-p", "hello", "--thinking", "low"]),
            Some((
                Command::Print {
                    prompt: "hello".into(),
                    images: Vec::new(),
                },
                Options {
                    model: None,
                    thinking_level: Some("low".into()),
                    continue_session: false,
                    config_dir: None,
                }
            ))
        );
    }
}
