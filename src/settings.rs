use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub check_for_updates: Option<bool>,
    pub notifications: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn settings_path() -> Result<PathBuf> {
    Ok(config::dir()
        .context("HOME is not set")?
        .join("settings.json"))
}

impl Settings {
    /// Reads settings.json, or the defaults when it is missing. When it
    /// cannot be read, update checks are off, so a typo cannot turn them on.
    pub fn load() -> Self {
        settings_path()
            .map(|path| load_file(&path))
            .unwrap_or_default()
    }

    /// Applies `change` to settings.json, keeping the settings it leaves
    /// alone. Fails instead of replacing a file it cannot read.
    pub fn update(change: impl FnOnce(&mut Self)) -> Result<()> {
        update_file(&settings_path()?, change)
    }
}

fn load_file(path: &Path) -> Settings {
    read_file(path).unwrap_or_else(|_| Settings {
        check_for_updates: Some(false),
        ..Settings::default()
    })
}

fn read_file(path: &Path) -> Result<Settings> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    serde_json::from_str(&text).map_err(|error| {
        anyhow::anyhow!(
            "{} could not be read: {error}; fix or delete it",
            path.display()
        )
    })
}

fn update_file(path: &Path, change: impl FnOnce(&mut Settings)) -> Result<()> {
    let mut settings = read_file(path)?;
    change(&mut settings);
    config::create_private_dir(path.parent().context("invalid settings path")?)?;
    config::write_private_file(path, serde_json::to_string_pretty(&settings)?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load_file, update_file};

    fn settings_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "rust-coding-agent-settings-{name}-{}",
                std::process::id()
            ))
            .join("settings.json")
    }

    fn written_settings_file(name: &str, text: &str) -> std::path::PathBuf {
        let path = settings_file(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn keeps_the_other_setting_when_one_changes() {
        let path = settings_file("keep");
        update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
        update_file(&path, |settings| settings.effort = Some("high".into())).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(saved["model"], "opus");
        assert_eq!(saved["effort"], "high");
    }

    #[test]
    fn refuses_to_replace_settings_that_are_not_valid_json() {
        let broken = "{ \"model\": \"opus\", }";
        let path = written_settings_file("broken", broken);
        let result = update_file(&path, |settings| settings.effort = Some("high".into()));
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
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
        let path = written_settings_file("broken-load", "{ \"check_for_updates\": false, }");
        let settings = load_file(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(settings.check_for_updates, Some(false));
    }

    #[test]
    fn checks_for_updates_when_settings_are_missing() {
        let path = settings_file("missing-load");
        assert_eq!(load_file(&path).check_for_updates, None);
    }

    #[test]
    fn keeps_settings_it_does_not_know_when_one_changes() {
        let path = written_settings_file("unknown", r#"{ "future": { "a": 1 } }"#);
        update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(saved["future"], serde_json::json!({ "a": 1 }));
        assert_eq!(saved["model"], "opus");
    }

    #[test]
    fn refuses_to_replace_settings_of_the_wrong_type() {
        let wrong = r#"{ "notifications": "yes" }"#;
        let path = written_settings_file("wrong-type", wrong);
        let result = update_file(&path, |settings| settings.model = Some("opus".into()));
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let error = format!("{:#}", result.unwrap_err());
        assert!(
            error.contains("settings.json could not be read: invalid type"),
            "{error}"
        );
        assert_eq!(text, wrong);
    }

    #[cfg(unix)]
    #[test]
    fn saves_settings_only_the_user_can_read() {
        use crate::config::permissions;
        use std::os::unix::fs::PermissionsExt;
        let path = written_settings_file("private", "{}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
        let found = permissions(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(found, Some(0o600));
    }
}
