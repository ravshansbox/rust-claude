use super::{Entry, MAX_ENTRIES, append, load, newest_unique, trim};
use crate::tools::test_support::TemporaryDir;
use std::io::Write;

fn history_file(name: &str) -> (TemporaryDir, std::path::PathBuf) {
    let directory = TemporaryDir::new(&format!("{name}history"));
    let path = directory.path().join("history.jsonl");
    (directory, path)
}

fn write_full_history(path: &std::path::Path) {
    let text: String = (0..MAX_ENTRIES + 5)
        .map(|index| format!("{{\"prompt\":\"p{index}\",\"cwd\":\"/project\"}}\n"))
        .collect();
    std::fs::write(path, text).unwrap();
}

fn append_raw(path: &std::path::Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

fn prompts(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.prompt.as_str()).collect()
}

#[test]
fn loads_newest_first_without_repeats() {
    let (_directory, path) = history_file("");
    for prompt in ["one", "two", "one", "three"] {
        append(&path, &Entry::here(prompt)).unwrap();
    }
    append_raw(&path, b"not json\n{\"prompt\":\"x\",\"cwd\":1}\n");
    let (text, entries) = (
        std::fs::read_to_string(&path).unwrap(),
        newest_unique(&load(&path)),
    );
    assert_eq!(prompts(&entries), ["three", "one", "two"]);
    let folder = std::env::current_dir().unwrap().display().to_string();
    assert!(text.starts_with(&format!("{{\"cwd\":{folder:?},\"prompt\":\"one\"}}\n")));
    assert!(entries.iter().all(|entry| entry.folder == folder));
}

#[test]
fn keeps_history_after_a_line_cut_short_inside_a_character() {
    let (_directory, path) = history_file("cut-");
    append(&path, &Entry::here("one")).unwrap();
    append_raw(&path, b"{\"prompt\":\"caf\xc3\n");
    append(&path, &Entry::here("two")).unwrap();
    let entries = load(&path);
    assert_eq!(prompts(&entries), ["one", "two"]);
}

#[test]
fn keeps_a_prompt_sent_after_a_line_cut_short_before_its_end() {
    let (_directory, path) = history_file("unended-");
    append(&path, &Entry::here("one")).unwrap();
    append_raw(&path, b"{\"prompt\":\"cut");
    append(&path, &Entry::here("two")).unwrap();
    let entries = load(&path);
    assert_eq!(prompts(&entries), ["one", "two"]);
}

#[test]
fn keeps_only_the_newest_prompts() {
    let (_directory, path) = history_file("long-");
    write_full_history(&path);
    let mut entries = load(&path);
    let trimmed = trim(&path, &mut entries);
    let reloaded = load(&path);
    trimmed.unwrap();
    for entries in [entries, reloaded] {
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].prompt, "p5");
        assert_eq!(entries[0].folder, "/project");
        assert_eq!(
            entries[MAX_ENTRIES - 1].prompt,
            format!("p{}", MAX_ENTRIES + 4)
        );
    }
}

#[test]
fn keeps_prompts_another_copy_sent_after_loading() {
    let (_directory, path) = history_file("shared-");
    write_full_history(&path);
    let mut entries = load(&path);
    // Another running copy sends a prompt after this one loaded.
    append(&path, &Entry::here("from other copy")).unwrap();
    let trimmed = trim(&path, &mut entries);
    let reloaded = load(&path);
    trimmed.unwrap();
    for entries in [entries, reloaded] {
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].prompt, "p6");
        assert_eq!(entries[MAX_ENTRIES - 1].prompt, "from other copy");
    }
}

#[cfg(unix)]
#[test]
fn saves_history_only_the_user_can_read() {
    use crate::config::permissions;
    let root = TemporaryDir::new("private-history");
    let directory = root.path().join("config");
    let path = directory.join("history.jsonl");
    append(&path, &Entry::here("secret")).unwrap();
    let found = (permissions(&directory), permissions(&path));
    assert_eq!(found, (Some(0o700), Some(0o600)));
}
