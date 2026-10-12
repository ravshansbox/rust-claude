use super::{file_query, files_in, walk};
use crate::tools::test_support::TemporaryDir;
use crate::tui::{
    keys::{Action, handle_input},
    test_support::{new_app, press, press_with},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use std::process::Command;

#[test]
fn lists_git_files_with_non_ascii_names() {
    let folder = TemporaryDir::new("files");
    let root = folder.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/café.rs"), "").unwrap();
    std::fs::write(root.join("plain.rs"), "").unwrap();
    let initialised = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success();
    let files = files_in(root);
    assert!(initialised);
    assert_eq!(files, ["plain.rs", "src/café.rs"]);
}

#[test]
fn skips_ignored_files_outside_git() {
    let folder = TemporaryDir::new("walk");
    let root = folder.path();
    for subfolder in ["target", ".hidden", "logs"] {
        std::fs::create_dir_all(root.join(subfolder)).unwrap();
    }
    for file in [
        "a.rs",
        "b.rs",
        "skip.log",
        "logs/out.txt",
        "target/out.rs",
        ".hidden/x.rs",
    ] {
        std::fs::write(root.join(file), "").unwrap();
    }
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(root.join(".ignore"), "logs/\n").unwrap();
    let files = files_in(root);
    assert_eq!(files, ["a.rs", "b.rs"]);
}

#[test]
fn stops_walking_at_the_file_limit() {
    let folder = TemporaryDir::new("limit");
    let root = folder.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    for file in ["a.rs", "b.rs", "src/c.rs", "src/d.rs"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let all = walk(root, 10);
    let capped = walk(root, 3);
    assert_eq!(all, ["a.rs", "b.rs", "src/c.rs", "src/d.rs"]);
    assert_eq!(capped.len(), 3);
}

#[test]
fn finds_file_query_at_cursor() {
    assert_eq!(file_query("read @src/ma", 12), Some((5, "src/ma")));
    assert_eq!(file_query("@", 1), Some((0, "")));
    assert_eq!(file_query("mail@host", 9), None);
    assert_eq!(file_query("@src now", 8), None);
}

#[test]
fn picks_file_after_at_sign() {
    let mut app = new_app();
    app.set_files(0, vec!["src/agent.rs".into(), "src/main.rs".into()]);
    for character in "read @src".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    press(&mut app, KeyCode::Down);
    let mut submitted = false;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |_| {
        submitted = true;
    });
    assert!(!submitted);
    assert_eq!(app.input, "read @src/main.rs ");
    assert_eq!(app.cursor, app.input.len());
    assert!(app.visible_suggestions().is_none());
}

#[test]
fn picks_file_with_tab() {
    let mut app = new_app();
    app.set_files(0, vec!["src/agent.rs".into(), "src/main.rs".into()]);
    handle_input(Event::Paste("read @src".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Down);
    let mut submitted = false;
    press_with(&mut app, KeyCode::Tab, KeyModifiers::NONE, |_| {
        submitted = true;
    });
    assert!(!submitted);
    assert_eq!(app.input, "read @src/main.rs ");
    assert_eq!(app.cursor, app.input.len());
    assert!(app.visible_suggestions().is_none());
}

#[test]
fn sends_the_typed_file_name_after_esc_hides_the_suggestions() {
    let mut app = new_app();
    app.set_files(0, vec!["src/main.rs".into()]);
    handle_input(Event::Paste("read @src".into()), &mut app, |_| {});
    let quit = press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |_| {});
    assert!(!quit);
    assert!(app.visible_suggestions().is_none());
    let mut submitted = Vec::new();
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Submit(prompt, _, _) = action {
            submitted.push(prompt);
        }
    });
    assert_eq!(submitted, ["read @src"]);
}

#[test]
fn replaces_the_whole_file_name_when_the_cursor_is_inside_it() {
    let mut app = new_app();
    app.set_files(0, vec!["src/main.rs".into()]);
    handle_input(Event::Paste("read @src/main.rs".into()), &mut app, |_| {});
    for _ in 0..2 {
        press(&mut app, KeyCode::Left);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, "read @src/main.rs ");
    assert_eq!(app.cursor, app.input.len());
}

#[test]
fn keeps_one_space_after_a_file_name_completed_before_other_text() {
    let mut app = new_app();
    app.set_files(0, vec!["src/main.rs".into()]);
    handle_input(Event::Paste("read @src please".into()), &mut app, |_| {});
    for _ in 0.." please".len() {
        press(&mut app, KeyCode::Left);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, "read @src/main.rs please");
    assert_eq!(app.cursor, "read @src/main.rs ".len());
}

#[test]
fn picks_the_file_named_exactly_before_longer_paths() {
    let mut app = new_app();
    app.set_files(
        0,
        vec![
            "crates/foo/src/lib.rs".into(),
            "crates/src/lib.rs.bak".into(),
            "src/lib.rs".into(),
            "src/lib.rs.orig".into(),
        ],
    );
    handle_input(Event::Paste("explain @SRC/lib".into()), &mut app, |_| {});
    let items: Vec<String> = app
        .visible_suggestions()
        .unwrap()
        .items
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        items,
        [
            "src/lib.rs",
            "src/lib.rs.orig",
            "crates/foo/src/lib.rs",
            "crates/src/lib.rs.bak"
        ]
    );
    handle_input(Event::Paste(".rs".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, "explain @src/lib.rs ");
}
