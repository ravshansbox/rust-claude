use std::{io::Write, path::PathBuf};

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
    pub fn load() -> Self {
        settings_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path()?;
        crate::config::create_private_dir(path.parent().context("invalid settings path")?)?;
        crate::config::private_file()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?
            .write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Settings, settings_path};

    #[cfg(unix)]
    #[test]
    fn saves_settings_only_the_user_can_read() {
        Settings::default().save().unwrap();
        let found = crate::config::permissions(&settings_path().unwrap());
        assert_eq!(found, Some(0o600));
    }
}
