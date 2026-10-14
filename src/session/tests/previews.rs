use crate::agent;
use crate::session::read_preview;
use crate::tools::test_support::TemporaryFile;
use serde_json::json;

#[test]
fn previews_prompt_with_images() {
    let file = TemporaryFile::new(
        "image-preview",
        "{\"role\":\"user\",\"content\":[{\"type\":\"image\"},{\"type\":\"text\",\"text\":\"look\"}]}\n",
    );
    assert_eq!(read_preview(file.path()).unwrap(), "look");
}

#[test]
fn previews_shell_command_without_markup() {
    let message = json!({
        "role": "user",
        "content": agent::shell_message("ls -la", "file\n"),
    });
    let file = TemporaryFile::new("shell-preview", &format!("{message}\n"));
    assert_eq!(read_preview(file.path()).unwrap(), "!ls -la");
}

#[test]
fn previews_skill_command_without_markup() {
    let block = "<skill name=\"demo\" location=\"/skills/demo/SKILL.md\">\nReferences are relative to /skills/demo.\n\nDo the demo.\n</skill>";
    for (content, preview) in [
        (format!("{block}\n\nfix it"), "/skill:demo fix it"),
        (block.to_string(), "/skill:demo"),
    ] {
        let message = json!({ "role": "user", "content": content });
        let file = TemporaryFile::new("skill-preview", &format!("{message}\n"));
        assert_eq!(read_preview(file.path()).unwrap(), preview);
    }
}

#[test]
fn reads_preview_without_reading_rest_of_file() {
    let file = TemporaryFile::new(
        "preview",
        "{\"role\":\"user\",\"content\":\"first\"}\nnot json\n",
    );
    assert_eq!(read_preview(file.path()).unwrap(), "first");
}
