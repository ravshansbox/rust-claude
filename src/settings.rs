use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    pub model: Option<String>,
    pub thinking_level: Option<String>,
}

fn settings_path() -> Result<PathBuf> {
    Ok(crate::config::dir()
        .context("HOME is not set")?
        .join("settings.json"))
}

impl Settings {
    /// Reads settings.json, or the defaults when it is missing or unreadable.
    pub fn load() -> Self {
        settings_path()
            .ok()
            .and_then(|path| read_file(&path).ok())
            .unwrap_or_default()
    }

    /// Applies `change` to settings.json, keeping the settings it leaves
    /// alone. Fails instead of replacing a file that is not valid JSON.
    pub fn update(change: impl FnOnce(&mut Settings)) -> Result<()> {
        update_file(&settings_path()?, change)
    }
}

fn read_file(path: &Path) -> Result<Settings> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON; fix or delete it", path.display()))
}

fn update_file(path: &Path, change: impl FnOnce(&mut Settings)) -> Result<()> {
    let mut settings = read_file(path)?;
    change(&mut settings);
    crate::config::create_private_dir(path.parent().context("invalid settings path")?)?;
    crate::config::write_private_file(path, serde_json::to_string_pretty(&settings)?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::update_file;

    fn settings_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "rust-claude-settings-{name}-{}",
                std::process::id()
            ))
            .join("settings.json")
    }

    #[test]
    fn keeps_the_other_setting_when_one_changes() {
        let path = settings_file("keep");
        update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
        update_file(&path, |settings| {
            settings.thinking_level = Some("high".into())
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(saved["model"], "opus");
        assert_eq!(saved["thinking_level"], "high");
    }

    #[test]
    fn refuses_to_replace_settings_that_are_not_valid_json() {
        let path = settings_file("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let broken = "{ \"model\": \"opus\", }";
        std::fs::write(&path, broken).unwrap();
        let result = update_file(&path, |settings| {
            settings.thinking_level = Some("high".into())
        });
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("settings.json is not valid JSON"), "{error}");
        assert_eq!(text, broken);
    }

    #[cfg(unix)]
    #[test]
    fn saves_settings_only_the_user_can_read() {
        use std::os::unix::fs::PermissionsExt;
        let path = settings_file("private");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        update_file(&path, |settings| settings.model = Some("opus".into())).unwrap();
        let found = crate::config::permissions(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(found, Some(0o600));
    }
}
