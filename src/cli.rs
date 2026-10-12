use anyhow::{Context, Result, bail};

pub const USAGE: &str = "usage: rust-coding-agent [-h|--help] [-c|--continue] [--config-dir <path>] [--model <id>] [--effort <level>] [-p|--print <prompt> [--image <path>]...]";

pub const OPTIONS: &str = "Options:
  -c, --continue           Continue the latest session in the current folder
      --config-dir <path>  Folder for sign-in, settings, sessions, history, skills and MCP config. Default: ~/.rust-coding-agent
      --model <id>         Model to use
      --effort <level>     Effort level: off (Sonnet and Haiku), low, medium, high, xhigh, max
  -p, --print <prompt>     Run one prompt and print the answer
      --image <path>       Send an image with the prompt in print mode. Repeat for more images
  -h, --help               Show this help";

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Interactive,
    Print { prompt: String, images: Vec<String> },
}

#[derive(Debug, Default, PartialEq)]
pub struct Options {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub continue_session: bool,
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

/// Reads arguments left to right, so a value such as the prompt after `-p`
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

#[cfg(test)]
mod tests {
    use super::{Command, Options, parse_arguments};

    fn parse_with_options(arguments: &[&str]) -> Option<(Command, Options)> {
        parse_arguments(arguments.iter().map(ToString::to_string).collect()).ok()
    }

    fn parse(arguments: &[&str]) -> Option<Command> {
        parse_with_options(arguments).map(|(command, _)| command)
    }

    #[test]
    fn rejects_help_with_other_arguments() {
        assert_eq!(parse(&["-h", "extra"]), None);
        assert_eq!(parse(&["--image", "a.png"]), None);
        assert_eq!(parse(&["-p", "hello", "--image"]), None);
        assert_eq!(parse(&["--help", "--model", "x"]), None);
        assert_eq!(parse(&["--model"]), None);
        assert_eq!(parse(&["--thinking", "high"]), None);
    }

    #[test]
    fn rejects_options_given_twice() {
        assert_eq!(parse(&["--model", "a", "--model", "b"]), None);
        assert_eq!(parse(&["--effort", "low", "--effort", "high"]), None);
        assert_eq!(parse(&["-p", "one", "--print", "two"]), None);
        assert_eq!(parse(&["-c", "--continue"]), None);
        assert_eq!(parse(&["--config-dir", "/a", "--config-dir", "/b"]), None);
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
    fn parses_model_and_effort_options() {
        assert_eq!(
            parse_with_options(&["--model", "claude-x", "--effort", "high"]),
            Some((
                Command::Interactive,
                Options {
                    model: Some("claude-x".into()),
                    effort: Some("high".into()),
                    continue_session: false,
                    config_dir: None,
                }
            ))
        );
        assert_eq!(
            parse_with_options(&["-p", "hello", "--effort", "low"]),
            Some((
                Command::Print {
                    prompt: "hello".into(),
                    images: Vec::new(),
                },
                Options {
                    model: None,
                    effort: Some("low".into()),
                    continue_session: false,
                    config_dir: None,
                }
            ))
        );
    }

    #[test]
    fn takes_a_prompt_that_looks_like_an_option() {
        for prompt in ["-c", "--model", "--image", "-h", "--help", "--config-dir"] {
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
