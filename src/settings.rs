use std::path::PathBuf;

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
        std::fs::create_dir_all(path.parent().context("invalid settings path")?)?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
