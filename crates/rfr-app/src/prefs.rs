// SPDX-License-Identifier: LGPL-3.0-or-later

//! Preferences kept between sessions (`prefs.json` in `rfr_core::paths::config_dir`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// UI font size in percent of the built-in sizes.
    pub font_percent: u32,
    /// Size of the whole interface (texts, controls, spacing) in percent.
    pub ui_scale_percent: u32,
    /// Settings sections the user closed (`widgets::section` keys).
    pub closed_sections: Vec<String>,
    /// Preview resolution in percent of the view size.
    pub preview_percent: u32,
    /// Resolution while navigating, in percent of the preview resolution.
    pub navigate_percent: u32,
    /// The preview stops refining after this many samples per pixel.
    pub preview_samples: u32,
    pub recent: Vec<PathBuf>,
    pub last_dir: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            font_percent: 100,
            ui_scale_percent: 100,
            closed_sections: Vec::new(),
            preview_percent: 100,
            navigate_percent: 50,
            preview_samples: 64,
            recent: Vec::new(),
            last_dir: None,
        }
    }
}

pub fn config_dir() -> PathBuf {
    rfr_core::paths::config_dir()
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(config_dir().join("prefs.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn add_recent(&mut self, p: PathBuf) {
        self.recent.retain(|r| *r != p);
        self.recent.insert(0, p);
        self.recent.truncate(12);
    }

    /// A file dialog in the last folder used.
    pub fn dialog(&self) -> rfd::FileDialog {
        let d = rfd::FileDialog::new();
        match &self.last_dir {
            Some(dir) if dir.is_dir() => d.set_directory(dir),
            _ => d,
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("prefs.json"), json).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_missing_fields_fall_back_to_defaults() {
        let p: Prefs = serde_json::from_str(r#"{"font_percent": 120, "future_field": 1}"#).unwrap();
        assert_eq!(p.font_percent, 120);
        assert_eq!(p.ui_scale_percent, 100);
    }
}
