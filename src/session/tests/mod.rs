use super::{
    Session, current_folder, images::inline_images, latest_in_folder, read::read_folder,
    sessions_directory,
};
use crate::images::Image;
use crate::tools::test_support::TemporaryDir;
use serde_json::json;
use std::{
    fs::File,
    path::PathBuf,
    time::{Duration, SystemTime},
};

mod listing;
mod previews;
mod resuming;

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
        File::options()
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
fn save_new(messages: &[serde_json::Value]) -> (String, PathBuf) {
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
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
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

#[cfg(unix)]
#[test]
fn saves_session_and_images_only_the_user_can_read() {
    use crate::config::test_support::permissions;
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
