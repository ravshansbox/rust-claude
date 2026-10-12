mod agent;
mod auth;
mod cli;
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
mod update;

use std::{
    io::{self, IsTerminal},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use cli::{Command, OPTIONS, USAGE, parse_arguments};
use tokio::signal::unix::{Signal, SignalKind, signal};

fn choose_model(saved: Option<String>) -> (String, Option<String>) {
    match saved {
        Some(model) if !models::is_known(&model) => (
            models::DEFAULT.into(),
            Some(format!(
                "{}, using {}",
                models::unknown(&model),
                models::DEFAULT
            )),
        ),
        saved => (saved.unwrap_or_else(|| models::DEFAULT.into()), None),
    }
}

fn choose_effort(model: &str, name: &str) -> (&'static str, Option<String>) {
    let levels = models::effort_levels(model);
    match levels.iter().find(|level| **level == name) {
        Some(level) => (level, None),
        None if name == "off" => (agent::DEFAULT_EFFORT, None),
        None => (
            agent::DEFAULT_EFFORT,
            Some(format!(
                "unknown effort level: {name} (options: {}), using {}",
                levels.join(", "),
                agent::DEFAULT_EFFORT
            )),
        ),
    }
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
    update::link_alias_beside_this_program();
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
    if let Some(model) = &options.model
        && !models::is_known(model)
    {
        bail!(models::unknown(model));
    }
    let (print_prompt, image_paths) = match command {
        Command::Help => {
            println!("A small coding agent for the terminal.\n\n{USAGE}\n\n{OPTIONS}");
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
    let model = options.model.unwrap_or_else(|| {
        let (model, warning) = choose_model(settings.model);
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
        model
    });
    let effort = options.effort.or(settings.effort);
    let mut agent = agent::Agent::new(http, credentials, model)?;
    if let Some(id) = resume_id {
        agent.resume(&id).await?;
    }
    if let Some(name) = effort {
        let (level, warning) = choose_effort(&agent.model, &name);
        agent.effort = level;
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
    }

    let Some(prompt) = print_prompt else {
        let result = tui::run(agent, stop).await;
        auth::finish_renewal(RENEWAL_WAIT).await;
        return result;
    };
    run_print(agent, stop, &prompt, &images).await
}

/// Runs the prompt given on the command line and prints the reply.
async fn run_print(
    mut agent: agent::Agent,
    mut stop: StopSignals,
    prompt: &str,
    images: &[images::Image],
) -> Result<()> {
    let stopped = stop.recv();
    tokio::pin!(stopped);
    tokio::select! {
        mcp = mcp::Mcp::load() => agent.mcp = mcp,
        status = &mut stopped => {
            if let Err(error) = agent.cancel_unsent(prompt, images) {
                eprintln!("failed to save session: {error}");
            }
            return interrupted(agent, status).await;
        }
    }
    for diagnostic in &agent.mcp.diagnostics {
        eprintln!("{}", highlight::strip_controls(diagnostic));
    }
    let stdout = io::stdout();
    let out_terminal = stdout.is_terminal();
    let colour = io::stderr().is_terminal();
    let (dark, background) = if colour {
        tui::terminal_theme()
    } else {
        (false, None)
    };
    let mut printer =
        print::Printer::new(stdout, out_terminal, io::stderr(), colour, dark, background);
    let mut checkpoint = None;
    let run = print::run(&mut agent, &mut printer, prompt, images, &mut checkpoint);
    let result = tokio::select! {
        result = run => Ok(result),
        status = &mut stopped => Err(status),
    };
    printer.flush_reads();
    let result = match result {
        Ok(result) => result,
        Err(status) => {
            if let Some(checkpoint) = checkpoint
                && let Err(error) = agent.cancel(checkpoint)
            {
                eprintln!("failed to save session: {error}");
            }
            return interrupted(agent, status).await;
        }
    };
    printer.finish(result)
}

/// Ctrl+C, a closed terminal (hangup) and `kill` (terminate) all stop
/// rust-coding-agent the same way, so running commands and MCP servers are stopped
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
const RENEWAL_WAIT: Duration = Duration::from_secs(5);

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
    use super::{choose_effort, choose_model};

    #[test]
    fn chooses_an_effort_level_the_model_supports() {
        assert_eq!(choose_effort("claude-haiku-5-5", "off"), ("off", None));
        assert_eq!(choose_effort("claude-opus-5-5", "high"), ("high", None));
        assert_eq!(choose_effort("claude-opus-5-5", "off"), ("medium", None));
        assert_eq!(
            choose_effort("claude-opus-5-5", "loud"),
            (
                "medium",
                Some(
                    "unknown effort level: loud (options: low, medium, high, xhigh, max), using medium"
                        .into()
                )
            )
        );
    }

    #[test]
    fn uses_the_default_model_when_the_saved_one_is_not_in_the_list() {
        assert_eq!(
            choose_model(Some("claude-haiku-5-5".into())),
            ("claude-haiku-5-5".into(), None)
        );
        assert_eq!(choose_model(None), ("claude-opus-5-5".into(), None));
        assert_eq!(
            choose_model(Some("claude-opus-4-5".into())),
            (
                "claude-opus-5-5".into(),
                Some(
                    "unknown model: claude-opus-4-5 (options: claude-fable-5-1, claude-opus-5-5, claude-sonnet-5-5, claude-haiku-5-5), using claude-opus-5-5"
                        .into()
                )
            )
        );
    }
}
