mod agent;
mod auth;
mod clipboard;
mod config;
mod highlight;
mod history;
mod images;
mod mcp;
mod models;
mod print;
mod session;
mod settings;
mod skills;
mod tools;
mod tui;

use std::{io::IsTerminal, time::Duration};

use anyhow::{Context, Result, bail};
use tokio::signal::unix::{Signal, SignalKind, signal};

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

/// Stores the value after an option, which may be given once.
fn set_value(
    slot: &mut Option<String>,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<()> {
    match (slot.is_none(), arguments.next()) {
        (true, Some(value)) => {
            *slot = Some(value);
            Ok(())
        }
        _ => bail!(USAGE),
    }
}

/// Reads arguments left to right, so a value such as the prompt after `-p`
/// is never mistaken for an option.
fn parse_arguments(arguments: Vec<String>) -> Result<(Command, Options)> {
    let mut arguments = arguments.into_iter();
    let mut options = Options::default();
    let mut images = Vec::new();
    let mut prompt = None;
    let mut help = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--image" => images.push(arguments.next().context(USAGE)?),
            "--model" => set_value(&mut options.model, &mut arguments)?,
            "--thinking" => set_value(&mut options.thinking_level, &mut arguments)?,
            "--config-dir" => {
                set_value(&mut options.config_dir, &mut arguments)?;
                // An empty folder, such as an unset variable, would put the
                // sign-in and sessions in the current folder.
                if options.config_dir.as_deref() == Some("") {
                    bail!(USAGE);
                }
            }
            "-p" | "--print" => set_value(&mut prompt, &mut arguments)?,
            "-c" | "--continue" if !options.continue_session => options.continue_session = true,
            "-h" | "--help" if !help => help = true,
            _ => bail!(USAGE),
        }
    }
    let command = match (help, prompt) {
        (true, None) if images.is_empty() && options == Options::default() => Command::Help,
        (false, None) if images.is_empty() => Command::Interactive,
        (false, Some(prompt)) => Command::Print { prompt, images },
        _ => bail!(USAGE),
    };
    Ok((command, options))
}

/// How long quitting waits for background jobs such as listing files for
/// `@`, reading the Git branch or reading the clipboard. Their results are
/// only shown in the interface, so a slow one should not hold up quitting,
/// but a file write cut short by quitting still gets a moment to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    result
}

async fn run() -> Result<()> {
    let arguments = std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument.into_string().map_err(|argument| {
                anyhow::anyhow!(
                    "argument is not valid UTF-8: {}\n{USAGE}",
                    argument.to_string_lossy()
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let (command, options) = parse_arguments(arguments)?;
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

    // Look the session up before signing in, so a missing one is reported
    // without asking to sign in first.
    let resume_id = if options.continue_session {
        Some(
            session::Session::latest_in_current_folder()?
                .context("no session to continue in this folder")?,
        )
    } else {
        None
    };
    // Signals are caught from here on, so stopping waits for a sign-in
    // renewal at start to be saved instead of losing the new tokens.
    let mut stop = StopSignals::new()?;
    let http = agent::http_client()?;
    let credentials = tokio::select! {
        credentials = auth::Credentials::load_or_login(&http) => credentials?,
        status = stop.recv() => return stopped(status).await,
    };
    let settings = settings::Settings::load();
    let model = options
        .model
        .or(settings.model)
        .unwrap_or_else(|| "claude-opus-5-5".into());
    let thinking_level = options.thinking_level.or(settings.thinking_level);
    let mut agent = agent::Agent::new(http, credentials, model)?;
    if let Some(id) = resume_id {
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
        let result = tui::run(agent, stop).await;
        auth::finish_renewal(RENEWAL_WAIT).await;
        return result;
    };
    let stopped = stop.recv();
    tokio::pin!(stopped);
    tokio::select! {
        mcp = mcp::Mcp::load() => agent.mcp = mcp,
        status = &mut stopped => {
            if let Err(error) = agent.cancel_unsent(&prompt, &images) {
                eprintln!("failed to save session: {error}");
            }
            return interrupted(agent, status).await;
        }
    }
    for diagnostic in &agent.mcp.diagnostics {
        eprintln!("{}", highlight::strip_controls(diagnostic));
    }
    for program in &agent.missing_programs {
        eprintln!("{} not found on PATH", highlight::strip_controls(program));
    }
    let stdout = std::io::stdout();
    let out_terminal = stdout.is_terminal();
    let colour = std::io::stderr().is_terminal();
    let dark = colour && tui::dark_theme();
    let mut printer = print::Printer::new(stdout, out_terminal, std::io::stderr(), colour, dark);
    let checkpoint = agent.history_len();
    let run = agent.prompt(&prompt, &images, |event| printer.event(event));
    let result = tokio::select! {
        result = run => Ok(result),
        status = &mut stopped => Err(status),
    };
    printer.flush_reads();
    let result = match result {
        Ok(result) => result,
        Err(status) => {
            let saved = agent.cancel(checkpoint);
            if let Err(error) = saved {
                eprintln!("failed to save session: {error}");
            }
            return interrupted(agent, status).await;
        }
    };
    printer.finish(result)?;
    Ok(())
}

/// Ctrl+C, a closed terminal (hangup) and `kill` (terminate) all stop
/// rust-claude the same way, so running commands and MCP servers are stopped
/// instead of left behind.
struct StopSignals {
    interrupt: Signal,
    hangup: Signal,
    terminate: Signal,
}

impl StopSignals {
    fn new() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            hangup: signal(SignalKind::hangup())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// Waits for a stop signal and returns the conventional exit status for
    /// it: 128 plus the signal number.
    async fn recv(&mut self) -> i32 {
        tokio::select! {
            _ = self.interrupt.recv() => 130,
            _ = self.hangup.recv() => 129,
            _ = self.terminate.recv() => 143,
        }
    }
}

/// How long stopping waits for a sign-in renewal to be saved.
const RENEWAL_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

async fn interrupted(agent: agent::Agent, status: i32) -> Result<()> {
    drop(agent);
    stopped(status).await
}

/// Exits after a stop signal, once a sign-in renewal in progress is saved.
async fn stopped(status: i32) -> Result<()> {
    auth::finish_renewal(RENEWAL_WAIT).await;
    eprintln!("\ncancelled");
    std::process::exit(status);
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
        assert_eq!(parse(&["--config-dir", ""]), None);
        assert_eq!(parse(&["--config-dir", "", "-p", "hello"]), None);
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

    #[test]
    fn takes_a_prompt_that_looks_like_an_option() {
        for prompt in ["-c", "--model", "--image", "-h", "--config-dir"] {
            assert_eq!(
                parse_with_options(&["-p", prompt]),
                Some((
                    Command::Print {
                        prompt: prompt.into(),
                        images: Vec::new(),
                    },
                    Options::default()
                ))
            );
        }
        assert_eq!(
            parse_with_options(&["--model", "-p", "-p", "-c"]),
            Some((
                Command::Print {
                    prompt: "-c".into(),
                    images: Vec::new(),
                },
                Options {
                    model: Some("-p".into()),
                    ..Options::default()
                }
            ))
        );
    }
}
