use crate::session::{Session, new_uuid, sessions_directory};
use serde_json::json;
use std::{
    fs::File,
    time::{Duration, SystemTime},
};

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
        File::options()
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

#[test]
fn leaves_out_subagent_sessions() {
    let parent = Session::new().unwrap();
    let mut subagent = Session::subagent(&parent.id).unwrap();
    subagent
        .save(&[json!({ "role": "user", "content": "task" })])
        .unwrap();
    let listed = Session::list_except("");
    std::fs::remove_dir_all(sessions_directory().unwrap().join(&parent.id)).unwrap();
    assert!(
        !listed
            .unwrap()
            .iter()
            .any(|summary| summary.id == subagent.id)
    );
}
