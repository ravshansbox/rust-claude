pub fn shell_message(command: &str, output: &str) -> String {
    format!("<bash-input>{command}</bash-input>\n<bash-output>{output}</bash-output>")
}

pub fn parse_shell_message(text: &str) -> Option<(&str, &str)> {
    let (command, rest) = text
        .strip_prefix("<bash-input>")?
        .split_once("</bash-input>\n<bash-output>")?;
    Some((command, rest.strip_suffix("</bash-output>")?))
}
