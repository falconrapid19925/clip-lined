//! TOML config + one-time legacy `clipdiary.cfg` import (subset).

use anyhow::Context;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub storage: Storage,
    pub paste: Paste,
    pub ui: Ui,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Storage {
    pub path: Option<std::path::PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Paste {
    pub default_mode: String,
}

impl Default for Paste {
    fn default() -> Self {
        Self {
            default_mode: "set_only".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub hide_after_ms: u64,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            hide_after_ms: 1200,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ok(toml::from_str(&text)?)
    }
}
