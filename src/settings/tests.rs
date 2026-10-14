use super::{load_file, update_file};
use crate::tools::test_support::TemporaryDir;
use std::path::PathBuf;

fn settings_file(name: &str) -> (TemporaryDir, PathBuf) {
    let directory = TemporaryDir::new(&format!("settings-{name}"));
    let path = directory.path().join("settings.json");
    (directory, path)
}

fn written_settings_file(name: &str, text: &str) -> (TemporaryDir, PathBuf) {
    let (directory, path) = settings_file(name);
    std::fs::write(&path, text).unwrap();
    (directory, path)
}

#[test]
fn keeps_the_other_setting_when_one_changes() {
    let (_directory, path) = settings_file("keep");
    update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
    update_file(&path, |settings| settings.effort = Some("high".into())).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(saved["model"], "opus");
    assert_eq!(saved["effort"], "high");
}

#[test]
fn refuses_to_replace_settings_that_are_not_valid_json() {
    let broken = "{ \"model\": \"opus\", }";
    let (_directory, path) = written_settings_file("broken", broken);
    let result = update_file(&path, |settings| settings.effort = Some("high".into()));
    let text = std::fs::read_to_string(&path).unwrap();
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("settings.json could not be read: "),
        "{error}"
    );
    assert!(error.ends_with("; fix or delete it"), "{error}");
    assert_eq!(text, broken);
}

#[test]
fn turns_update_checks_off_when_settings_cannot_be_read() {
    let (_directory, path) =
        written_settings_file("broken-load", "{ \"check_for_updates\": true, }");
    let settings = load_file(&path);
    assert_eq!(settings.check_for_updates, Some(false));
}

#[test]
fn keeps_settings_it_does_not_know_when_one_changes() {
    let (_directory, path) = written_settings_file("unknown", r#"{ "future": { "a": 1 } }"#);
    update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(saved["future"], serde_json::json!({ "a": 1 }));
    assert_eq!(saved["model"], "opus");
}

#[cfg(unix)]
#[test]
fn saves_settings_only_the_user_can_read() {
    use crate::config::test_support::permissions;
    use std::os::unix::fs::PermissionsExt;
    let (_directory, path) = written_settings_file("private", "{}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
    let found = permissions(&path);
    assert_eq!(found, Some(0o600));
}
