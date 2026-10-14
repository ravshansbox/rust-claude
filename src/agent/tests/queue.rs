use serde_json::json;
use std::sync::Arc;

use crate::agent::{
    Queued,
    test_support::{self, MockApi, text_reply, tool_reply},
};
use crate::images::Image;

#[tokio::test]
async fn lets_prompts_be_queued_while_queued_images_are_saved() {
    use std::sync::{
        TryLockError,
        atomic::{AtomicBool, Ordering},
    };

    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "true" })),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let small = Image {
        media_type: "image/png",
        data: vec![1; 16],
    };
    // Large enough that saving it takes a while.
    let large = Image {
        media_type: "image/png",
        data: vec![2; 8 << 20],
    };
    agent.queue.lock().unwrap().push(Queued {
        prompt: "look at these".into(),
        images: vec![(1, small), (2, large)],
    });
    let directory = test_support::session_file(&agent).with_file_name(&agent.session.id);
    let queue = agent.queue.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let watching = finished.clone();
    // Watches the queue while the first image is saved and the second
    // one is being saved.
    let watcher = std::thread::spawn(move || {
        let mut blocked = false;
        while !watching.load(Ordering::Relaxed) {
            let saved = std::fs::read_dir(&directory).map_or(0, |entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension() == Some("png".as_ref()))
                    .count()
            });
            if saved == 1 && matches!(queue.try_lock(), Err(TryLockError::WouldBlock)) {
                blocked = true;
            }
        }
        blocked
    });
    let result = agent.prompt("run it", &[], |_| {}).await;
    finished.store(true, Ordering::Relaxed);
    let blocked = watcher.join().unwrap();
    test_support::remove_session(&agent);
    result.unwrap();
    assert!(!blocked, "the queue was locked while an image was saved");
    assert!(agent.queue.lock().unwrap().is_empty());
    let requests = api.requests().await;
    assert!(
        requests[1]["messages"]
            .to_string()
            .contains("look at these")
    );
}

#[tokio::test]
async fn keeps_prompts_queued_when_their_image_cannot_be_saved() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "true" })),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    let image = Image {
        media_type: "image/png",
        data: vec![1; 16],
    };
    agent.queue.lock().unwrap().push(Queued {
        prompt: "look at this".into(),
        images: vec![(1, image)],
    });
    // A file in the way of the session's image folder makes saving fail.
    let images = test_support::session_file(&agent).with_file_name(&agent.session.id);
    std::fs::create_dir_all(images.parent().unwrap()).unwrap();
    std::fs::write(&images, "").unwrap();
    let result = agent.prompt("run it", &[], |_| {}).await;
    let _ = std::fs::remove_file(&images);
    test_support::remove_session(&agent);
    assert!(result.is_err());
    let queued: Vec<_> = agent
        .queue
        .lock()
        .unwrap()
        .iter()
        .map(|queued| queued.prompt.clone())
        .collect();
    assert_eq!(queued, ["look at this"]);
}

#[tokio::test]
async fn keeps_the_conversation_cached_when_a_queued_prompt_follows_a_tool_round() {
    let api = MockApi::start(vec![
        tool_reply("call-1", "bash", &json!({ "command": "true" })),
        text_reply("done"),
    ])
    .await;
    let mut agent = test_support::agent(&api);
    agent.queue.lock().unwrap().push(Queued {
        prompt: "also this".into(),
        images: Vec::new(),
    });
    let result = agent.prompt("run it", &[], |_| {}).await;
    test_support::remove_session(&agent);
    result.unwrap();
    let requests = api.requests().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    let marked: Vec<bool> = messages
        .iter()
        .map(|message| message.to_string().contains("cache_control"))
        .collect();
    // The prompt ended the first request, where its cache entry was written.
    assert_eq!(marked, [true, false, false, true]);
}
