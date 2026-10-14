use super::{Command, Options, parse_arguments};

fn parse_with_options(arguments: &[&str]) -> Option<(Command, Options)> {
    parse_arguments(arguments.iter().map(ToString::to_string).collect()).ok()
}

fn parse(arguments: &[&str]) -> Option<Command> {
    parse_with_options(arguments).map(|(command, _)| command)
}

#[test]
fn rejects_help_with_other_arguments() {
    assert_eq!(parse(&["--help", "extra"]), None);
    assert_eq!(parse(&["--image", "a.png"]), None);
    assert_eq!(parse(&["--print", "hello", "--image"]), None);
    assert_eq!(parse(&["--help", "--model", "x"]), None);
    assert_eq!(parse(&["--model"]), None);
    assert_eq!(parse(&["--thinking", "high"]), None);
}

#[test]
fn rejects_short_options() {
    assert_eq!(parse(&["-p", "hello"]), None);
    assert_eq!(parse(&["-c"]), None);
    assert_eq!(parse(&["-h"]), None);
}

#[test]
fn rejects_options_given_twice() {
    assert_eq!(parse(&["--model", "a", "--model", "b"]), None);
    assert_eq!(parse(&["--effort", "low", "--effort", "high"]), None);
    assert_eq!(parse(&["--print", "one", "--print", "two"]), None);
    assert_eq!(parse(&["--continue", "--continue"]), None);
    assert_eq!(parse(&["--config-dir", "/a", "--config-dir", "/b"]), None);
}

#[test]
fn parses_print_and_interactive() {
    assert_eq!(parse(&[]), Some(Command::Interactive));
    assert_eq!(
        parse(&["--image", "a.png", "--print", "hello", "--image", "b.png"]),
        Some(Command::Print {
            prompt: "hello".into(),
            images: vec!["a.png".into(), "b.png".into()],
        })
    );
}

#[test]
fn parses_continue_option() {
    assert_eq!(
        parse_with_options(&["--continue"]),
        Some((
            Command::Interactive,
            Options {
                continue_session: true,
                ..Options::default()
            }
        ))
    );
    assert_eq!(
        parse_with_options(&["--continue", "--print", "hello"])
            .map(|(_, options)| options.continue_session),
        Some(true)
    );
    assert_eq!(parse(&["--continue", "--help"]), None);
}

#[test]
fn parses_config_dir_option() {
    assert_eq!(
        parse_with_options(&["--config-dir", "/tmp/rc", "--print", "hello"])
            .map(|(_, options)| options.config_dir),
        Some(Some("/tmp/rc".into()))
    );
    assert_eq!(parse(&["--config-dir"]), None);
    assert_eq!(parse(&["--config-dir", ""]), None);
    assert_eq!(parse(&["--config-dir", "", "--print", "hello"]), None);
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
        parse_with_options(&["--print", "hello", "--effort", "low"]),
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
    for prompt in [
        "-p",
        "--continue",
        "--model",
        "--image",
        "--help",
        "--config-dir",
    ] {
        assert_eq!(
            parse_with_options(&["--print", prompt]),
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
        parse_with_options(&["--model", "--print", "--print", "--continue"]),
        Some((
            Command::Print {
                prompt: "--continue".into(),
                images: Vec::new(),
            },
            Options {
                model: Some("--print".into()),
                ..Options::default()
            }
        ))
    );
}
