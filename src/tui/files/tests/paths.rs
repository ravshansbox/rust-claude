use super::*;
use crate::tui::files::{folder_path, list_folder};
use std::path::{Path, PathBuf};

fn load_folder(app: &mut crate::tui::App) {
    if let Some((generation, folder)) = app.start_listing_folder() {
        let entries = list_folder(&folder);
        handle_agent_event(UiEvent::Folder(generation, folder, entries), app);
    }
}

fn items(app: &crate::tui::App) -> Vec<String> {
    app.visible_suggestions()
        .map(|suggestions| {
            suggestions
                .items
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        })
        .unwrap_or_default()
}

fn folder_with_files(name: &str) -> TemporaryDir {
    let folder = TemporaryDir::new(name);
    let root = folder.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    for file in ["Notes.md", "a.rs", ".env", "src/main.rs"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    folder
}

#[test]
fn lists_the_folder_typed_after_at_sign() {
    let folder = folder_with_files("paths-lists");
    let root = folder.path().display().to_string();
    let mut app = new_app();
    handle_input(Event::Paste(format!("read @{root}/")), &mut app, |_| {});
    load_folder(&mut app);
    assert_eq!(
        items(&app),
        [
            format!("{root}/Notes.md"),
            format!("{root}/a.rs"),
            format!("{root}/src/"),
        ]
    );
    assert!(screen(&mut app).contains("src/"));
}

#[test]
fn filters_folder_entries_by_the_typed_name() {
    let folder = folder_with_files("paths-filters");
    let root = folder.path().display().to_string();
    let mut app = new_app();
    handle_input(Event::Paste(format!("@{root}/n")), &mut app, |_| {});
    load_folder(&mut app);
    assert_eq!(items(&app), [format!("{root}/Notes.md")]);
    handle_input(Event::Paste(format!(" @{root}/.")), &mut app, |_| {});
    load_folder(&mut app);
    assert_eq!(items(&app), [format!("{root}/.env")]);
}

#[test]
fn keeps_listing_after_picking_a_folder() {
    let folder = folder_with_files("paths-keeps");
    let root = folder.path().display().to_string();
    let mut app = new_app();
    handle_input(Event::Paste(format!("read @{root}/s")), &mut app, |_| {});
    load_folder(&mut app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, format!("read @{root}/src/"));
    load_folder(&mut app);
    assert_eq!(items(&app), [format!("{root}/src/main.rs")]);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.input, format!("read @{root}/src/main.rs "));
    assert!(app.visible_suggestions().is_none());
}

#[test]
fn resolves_home_and_relative_folders() {
    let home = Some(PathBuf::from("/home/me"));
    assert_eq!(folder_path("~/", home.clone()), Path::new("/home/me/"));
    assert_eq!(
        folder_path("~/src/", home.clone()),
        Path::new("/home/me/src/")
    );
    assert_eq!(folder_path("../", home.clone()), Path::new("../"));
    assert_eq!(folder_path("./src/", home.clone()), Path::new("./src/"));
    assert_eq!(folder_path("/etc/", home), Path::new("/etc/"));
}

#[test]
fn drops_a_folder_list_started_before_the_prompt_was_sent() {
    let folder = folder_with_files("paths-drops");
    let root = folder.path().display().to_string();
    let mut app = new_app();
    handle_input(Event::Paste(format!("@{root}/")), &mut app, |_| {});
    let (generation, listed) = app.start_listing_folder().unwrap();
    press(&mut app, KeyCode::Enter);
    handle_agent_event(
        UiEvent::Folder(generation, listed, list_folder(&format!("{root}/"))),
        &mut app,
    );
    handle_input(Event::Paste(format!("@{root}/")), &mut app, |_| {});
    assert!(items(&app).is_empty());
    assert!(app.start_listing_folder().is_some());
}
