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
mod restart;
mod session;
mod settings;
mod skills;
mod stop;
mod tools;
mod tui;
mod update;

use std::{
    io::{self, IsTerminal},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use cli::{Command, OPTIONS, USAGE, command_line_arguments, parse_arguments, read_stdin_prompt};
use stop::{RENEWAL_WAIT, StopSignals, stopped};

/// How long quitting waits for background jobs such as listing files for
/// `@`, reading the Git branch or reading the clipboard. Their results are
/// only shown in the interface, so a slow one should not hold up quitting,
/// but a file write cut short by quitting still gets a moment to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> Result<()> {
    let restarted_input = restart::take_input();
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(run(restarted_input));
    runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    match result? {
        Some(restart) => restart.run(),
        None => Ok(()),
    }
}

async fn run(restarted_input: Option<String>) -> Result<Option<restart::Restart>> {
    update::link_alias_beside_this_program();
    let (command, options) = parse_arguments(command_line_arguments()?)?;
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
            return Ok(None);
        }
        Command::Interactive if io::stdin().is_terminal() => (None, Vec::new()),
        Command::Interactive => (Some(read_stdin_prompt()?), Vec::new()),
        Command::Print { prompt, images } => (Some(prompt), images),
    };
    let images = load_images(&image_paths)?;

    // Look the session up before signing in, so a missing one is reported
    // without asking to sign in first.
    let resume_id = if let Some(id) = &options.resume {
        Some(session::Session::find(id)?)
    } else if options.continue_session {
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
        status = stop.recv() => return stopped(status).await.map(|()| None),
    };
    let settings = settings::Settings::load();
    let model = options.model.unwrap_or_else(|| {
        let (model, warning) = models::choose(settings.model);
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
        let (level, warning) = models::choose_effort(&agent.model, &name);
        agent.effort = level;
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
    }

    let Some(prompt) = print_prompt else {
        let result = tui::run(agent, stop, restarted_input).await;
        auth::finish_renewal(RENEWAL_WAIT).await;
        return result;
    };
    run_print(agent, stop, &prompt, &images)
        .await
        .map(|()| None)
}

fn load_images(paths: &[String]) -> Result<Vec<images::Image>> {
    paths
        .iter()
        .map(|path| {
            std::fs::read(path)
                .map_err(anyhow::Error::from)
                .and_then(images::prepare)
                .with_context(|| format!("failed to load image {path}"))
        })
        .collect()
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
            drop(agent);
            return self::stopped(status).await;
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
            drop(agent);
            return self::stopped(status).await;
        }
    };
    printer.finish(result)
}
