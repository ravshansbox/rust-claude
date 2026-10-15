use super::save_new;
use crate::session::Session;
use serde_json::json;
use std::io::Write;

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
