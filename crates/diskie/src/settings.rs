//! Settings, kept as JSON in `%APPDATA%\Diskie\settings.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Download and install in the background; the new version runs from the next start.
    Auto,
    /// Show a notice in the status bar (the default).
    #[default]
    Notify,
    Off,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub updates: UpdateMode,
    /// Unix seconds of the last automatic update check.
    pub last_update_check: u64,
    /// Share of the content height given to the treemap.
    pub treemap_share: f32,
    /// Read the NTFS Master File Table when running as administrator.
    pub use_mft: bool,
    /// Ask for administrator rights at startup (Windows).
    pub run_as_admin: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { updates: UpdateMode::Notify, last_update_check: 0, treemap_share: 0.46, use_mft: true, run_as_admin: true }
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    })?;
    Some(base.join("Diskie").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, serde_json::to_vec_pretty(self).unwrap_or_default());
        }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
