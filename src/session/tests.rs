use super::{
    Image, Session, agent, current_folder, inline_images, latest_in_folder, new_uuid, read_folder,
    read_preview, sessions_directory,
};
use crate::tools::test_support::{TemporaryDir, TemporaryFile};
use serde_json::json;
use std::{
    io::Write,
    time::{Duration, SystemTime},
};

#[test]
fn inlines_saved_images_as_base64() {
    let directory = TemporaryDir::new("images");
    std::fs::write(directory.path().join("a.png"), b"abc").unwrap();
    let mut messages = vec![json!({
        "role": "user",
        "content": [
            { "type": "image", "file": "a.png", "media_type": "image/png" },
            { "type": "text", "text": "look" },
        ],
    })];
    inline_images(directory.path(), &mut messages).unwrap();
    assert_eq!(
        messages[0]["content"][0],
        json!({
            "type": "image",
            "source": { "type": "base64", "media_type": "image/png", "data": "YWJj" },
        })
    );
}

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

#[test]
fn continues_latest_session_in_current_folder() {
    let directory = TemporaryDir::new("latest");
    let now = SystemTime::now();
    for (name, folder, age) in [
        ("old.jsonl", "/project", 60),
        ("new.jsonl", "/project", 30),
        ("other.jsonl", "/other", 0),
        ("notes.txt", "/project", 0),
    ] {
        let path = directory.path().join(name);
        std::fs::write(
            &path,
            format!("{}\n", json!({ "type": "session", "cwd": folder })),
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(now - Duration::from_secs(age)))
            .unwrap();
    }
    let latest = latest_in_folder(directory.path(), "/project");
    assert_eq!(latest.unwrap().as_deref(), Some("new"));
}

/// Saves the messages to a new session and closes it, returning its id
/// and file.
fn save_new(messages: &[serde_json::Value]) -> (String, std::path::PathBuf) {
    let mut session = Session::new().unwrap();
    session.save(messages).unwrap();
    let path = sessions_directory()
        .unwrap()
        .join(format!("{}.jsonl", session.id));
    (session.id.clone(), path)
}

#[test]
fn saves_session_folder_and_messages() {
    let messages = vec![json!({ "role": "user", "content": "hello" })];
    let (id, path) = save_new(&messages);
    let loaded = Session::load(&id);
    let folder = read_folder(&path);
    std::fs::remove_file(&path).unwrap();
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(!path.starts_with(home.join(".rust-coding-agent")));
    assert_eq!(loaded.unwrap().1, messages);
    assert_eq!(folder.unwrap(), Some(current_folder()));
}

#[test]
fn refuses_a_session_open_elsewhere() {
    let messages = vec![json!({ "role": "user", "content": "hello" })];
    let mut session = Session::new().unwrap();
    session.save(&messages).unwrap();
    let while_saving = Session::load(&session.id).map(|_| ());
    let id = session.id.clone();
    drop(session);
    let first = Session::load(&id);
    let second = Session::load(&id).map(|_| ());
    drop(first);
    let after_closing = Session::load(&id).map(|(_, loaded)| loaded);
    std::fs::remove_file(sessions_directory().unwrap().join(format!("{id}.jsonl"))).unwrap();
    for refused in [while_saving, second] {
        assert_eq!(
            refused.unwrap_err().to_string(),
            "this session is open in another rust-coding-agent"
        );
    }
    assert_eq!(after_closing.unwrap(), messages);
}

fn append_to_file(path: &std::path::Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .unwrap();
}

#[test]
fn resumes_messages_saved_before_a_line_cut_short() {
    let messages = vec![
        json!({ "role": "user", "content": "hello" }),
        json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] }),
    ];
    let (id, path) = save_new(&messages);
    // A line cut short in the middle of the two bytes of "é".
    append_to_file(&path, b"{\"role\":\"user\",\"content\":\"caf\xc3");
    let mut more = messages.clone();
    more.push(json!({ "role": "user", "content": "again" }));
    let resumed = Session::load(&id).and_then(|(mut resumed, loaded)| {
        resumed.save(&more)?;
        Ok(loaded)
    });
    let reloaded = Session::load(&id);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(resumed.unwrap(), messages);
    assert_eq!(reloaded.unwrap().1, more);
}

#[test]
fn resumes_messages_after_blank_lines() {
    let (id, path) = save_new(&[json!({ "role": "user", "content": "hello" })]);
    append_to_file(
        &path,
        b"\n  \n{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}\n",
    );
    let loaded = Session::load(&id);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        loaded.unwrap().1,
        [
            json!({ "role": "user", "content": "hello" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "hi" }] }),
        ]
    );
}

#[test]
fn drops_tool_call_whose_results_were_cut_short() {
    let messages = vec![
        json!({ "role": "user", "content": "run it" }),
        json!({ "role": "assistant", "content": [
            { "type": "tool_use", "id": "t1", "name": "bash", "input": {} },
        ] }),
    ];
    let (id, path) = save_new(&messages);
    append_to_file(
        &path,
        b"{\"role\":\"user\",\"content\":[{\"type\":\"tool_res",
    );
    let loaded = Session::load(&id);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(loaded.unwrap().1, messages[..1]);
}

#[test]
fn drops_tool_call_whose_results_were_not_written() {
    let messages = vec![
        json!({ "role": "user", "content": "run it" }),
        json!({ "role": "assistant", "content": [
            { "type": "tool_use", "id": "t1", "name": "bash", "input": {} },
        ] }),
    ];
    let (id, path) = save_new(&messages);
    let mut more = messages[..1].to_vec();
    more.push(json!({ "role": "user", "content": "again" }));
    let resumed = Session::load(&id).and_then(|(mut resumed, loaded)| {
        resumed.save(&more)?;
        Ok(loaded)
    });
    let reloaded = Session::load(&id);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(resumed.unwrap(), messages[..1]);
    assert_eq!(reloaded.unwrap().1, more);
}

#[cfg(unix)]
#[test]
fn saves_session_and_images_only_the_user_can_read() {
    use crate::config::permissions;
    let mut session = Session::new().unwrap();
    session
        .save(&[json!({ "role": "user", "content": "secret" })])
        .unwrap();
    let image = session.save_image(&Image {
        media_type: "image/png",
        data: b"png".to_vec(),
    });
    let file = sessions_directory()
        .unwrap()
        .join(format!("{}.jsonl", session.id));
    let images = sessions_directory().unwrap().join(&session.id);
    let image_file = image.map(|block| images.join(block["file"].as_str().unwrap()));
    let found = (
        permissions(&file),
        permissions(&images),
        permissions(&image_file.unwrap()),
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::remove_dir_all(&images).unwrap();
    assert_eq!(found, (Some(0o600), Some(0o700), Some(0o600)));
}

#[test]
fn replaces_an_image_a_failed_save_cut_short() {
    let session = Session::new().unwrap();
    let image = Image {
        media_type: "image/png",
        data: b"png".to_vec(),
    };
    let block = session.save_image(&image).unwrap();
    let images = sessions_directory().unwrap().join(&session.id);
    std::fs::write(images.join(block["file"].as_str().unwrap()), b"pn").unwrap();
    let mut messages = vec![json!({
        "role": "user",
        "content": [session.save_image(&image).unwrap()],
    })];
    let result = session.inline_images(&mut messages);
    std::fs::remove_dir_all(&images).unwrap();
    result.unwrap();
    assert_eq!(messages[0]["content"][0]["source"]["data"], "cG5n");
}

#[test]
fn does_not_create_file_without_messages() {
    let mut session = Session::new().unwrap();
    session.save(&[]).unwrap();
    let path = sessions_directory()
        .unwrap()
        .join(format!("{}.jsonl", session.id));
    assert!(!path.exists());
}

#[test]
fn skips_unreadable_session_files() {
    let session = Session::new().unwrap();
    let mut readable = Session::new().unwrap();
    readable
        .save(&[json!({ "role": "user", "content": "hello" })])
        .unwrap();
    let directory = sessions_directory().unwrap();
    let unreadable = new_uuid().unwrap();
    let path = directory.join(format!("{unreadable}.jsonl"));
    std::fs::write(&path, "not json\n").unwrap();
    let listed = Session::list_except(&session.id);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(directory.join(format!("{}.jsonl", readable.id))).unwrap();
    let ids: Vec<String> = listed
        .unwrap()
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    assert!(ids.contains(&readable.id));
    assert!(!ids.contains(&unreadable));
}

#[test]
fn lists_other_sessions_newest_first() {
    let directory = sessions_directory().unwrap();
    let mut sessions = Vec::new();
    for (prompt, age) in [("older", 120), ("newer", 60), ("current", 0)] {
        let mut session = Session::new().unwrap();
        session
            .save(&[json!({ "role": "user", "content": prompt })])
            .unwrap();
        let path = directory.join(format!("{}.jsonl", session.id));
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
        sessions.push((session.id.clone(), path));
    }
    let listed = Session::list_except(&sessions[2].0);
    for (_, path) in &sessions {
        std::fs::remove_file(path).unwrap();
    }
    let previews: Vec<String> = listed
        .unwrap()
        .into_iter()
        .filter(|summary| sessions.iter().any(|(id, _)| *id == summary.id))
        .map(|summary| summary.preview)
        .collect();
    assert_eq!(previews, ["newer", "older"]);
}
