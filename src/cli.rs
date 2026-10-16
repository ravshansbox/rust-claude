use anyhow::{Context, Result, bail};
use std::io;

pub const USAGE: &str = "usage: rust-coding-agent [--help] [--continue | --resume <id>] [--config-dir <path>] [--model <id>] [--effort <level>] [--print <prompt> [--image <path>]...]";

pub const OPTIONS: &str = "Options:
  --continue           Continue the latest session in the current folder
  --resume <id>        Continue the session with this id
  --config-dir <path>  Folder for sign-in, settings, sessions, history, skills and MCP config. Default: ~/.rust-coding-agent
  --model <id>         Model to use
  --effort <level>     Effort level: off (Sonnet and Haiku), low, medium, high, xhigh, max
  --print <prompt>     Run one prompt and print the answer. Without it, a prompt piped to standard input runs the same way
  --image <path>       Send an image with the prompt in print mode. Repeat for more images
  --help               Show this help";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Interactive,
    Print { prompt: String, images: Vec<String> },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub continue_session: bool,
    pub resume: Option<String>,
    pub config_dir: Option<String>,
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

/// Reads arguments left to right, so a value such as the prompt after `--print`
/// is never mistaken for an option.
pub fn parse_arguments(arguments: Vec<String>) -> Result<(Command, Options)> {
    let mut arguments = arguments.into_iter();
    let mut options = Options::default();
    let mut images = Vec::new();
    let mut prompt = None;
    let mut help = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--image" => images.push(arguments.next().context(USAGE)?),
            "--model" => set_value(&mut options.model, &mut arguments)?,
            "--effort" => set_value(&mut options.effort, &mut arguments)?,
            "--config-dir" => {
                set_value(&mut options.config_dir, &mut arguments)?;
                // An empty folder, such as an unset variable, would put the
                // sign-in and sessions in the current folder.
                if options.config_dir.as_deref() == Some("") {
                    bail!(USAGE);
                }
            }
            "--print" => {
                set_value(&mut prompt, &mut arguments)?;
                if prompt
                    .as_deref()
                    .is_some_and(|prompt| prompt.trim().is_empty())
                {
                    bail!("no prompt after --print\n{USAGE}");
                }
            }
            "--resume" if !options.continue_session => {
                set_value(&mut options.resume, &mut arguments)?;
            }
            "--continue" if !options.continue_session && options.resume.is_none() => {
                options.continue_session = true;
            }
            "--help" if !help => help = true,
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

pub fn command_line_arguments() -> Result<Vec<String>> {
    std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument.into_string().map_err(|argument| {
                anyhow::anyhow!(
                    "argument is not valid UTF-8: {}\n{USAGE}",
                    argument.to_string_lossy()
                )
            })
        })
        .collect()
}

pub fn read_stdin_prompt() -> Result<String> {
    let input = io::read_to_string(io::stdin()).context("failed to read standard input")?;
    let prompt = input.trim_end();
    if prompt.is_empty() {
        bail!("no prompt on standard input\n{USAGE}");
    }
    Ok(prompt.to_string())
}

#[cfg(test)]
mod tests;
