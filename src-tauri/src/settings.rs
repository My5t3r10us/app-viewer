//! User preferences (Paramètres page), stored in settings.json next to apps.json.

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Closing the window hides it to the tray instead of quitting (apps keep running).
    pub keep_in_background: bool,
    /// When the sidebar is hidden, hovering the left edge reveals it.
    pub reveal_sidebar_on_hover: bool,
    /// Seconds before a detached terminal closes once its command ended normally.
    pub terminal_autoclose_secs: u8,
    /// Page zoom factor of each app (1.0 = 100 %), by app id. Owned by the backend: the UI
    /// never sends it back (see `save_settings`).
    pub zoom: HashMap<String, f64>,
    /// Where `appv screenshot` saves its images; empty = %LOCALAPPDATA%\App Viewer\screenshots.
    pub screenshots_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { keep_in_background: false, reveal_sidebar_on_hover: true, terminal_autoclose_secs: 3, zoom: HashMap::new(), screenshots_dir: String::new() }
    }
}

pub fn load(path: &Path) -> Settings {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}
