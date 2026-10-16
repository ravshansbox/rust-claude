use std::{path::PathBuf, process::Command};

use anyhow::{Context, Result};

/// Holds the unsent input across a restart after an update.
pub const INPUT_VARIABLE: &str = "RUST_CODING_AGENT_RESTART_INPUT";

/// Starting the installed update in place of this program.
pub struct Restart {
    pub program: PathBuf,
    pub session: Option<String>,
    pub model: String,
    pub effort: &'static str,
    pub input: String,
    pub config_dir: Option<PathBuf>,
}

impl Restart {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        if let Some(config_dir) = &self.config_dir {
            command.arg("--config-dir").arg(config_dir);
        }
        if let Some(session) = &self.session {
            command.args(["--resume", session]);
        }
        command.args(["--model", &self.model, "--effort", self.effort]);
        command.env(INPUT_VARIABLE, &self.input);
        command
    }

    /// Replaces this process with the update.
    #[cfg(unix)]
    pub fn run(self) -> Result<()> {
        use std::os::unix::process::CommandExt;
        let error = self.command().exec();
        Err(error).with_context(|| format!("failed to restart {}", self.program.display()))
    }

    /// Runs the update until it exits, then exits with its status.
    #[cfg(not(unix))]
    pub fn run(self) -> Result<()> {
        let status = self
            .command()
            .status()
            .with_context(|| format!("failed to restart {}", self.program.display()))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// The input left unsent before a restart, if this run is one. Call it
/// before starting other threads, as it changes the environment.
pub fn take_input() -> Option<String> {
    let input = std::env::var(INPUT_VARIABLE).ok()?;
    // SAFETY: called at start, before any other thread runs.
    unsafe { std::env::remove_var(INPUT_VARIABLE) };
    Some(input)
}

#[cfg(test)]
mod tests;
