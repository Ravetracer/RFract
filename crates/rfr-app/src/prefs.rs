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
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { font_percent: 100, ui_scale_percent: 100, closed_sections: Vec::new() }
    }
}

pub fn config_dir() -> PathBuf {
    rfr_core::paths::config_dir()
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(config_dir().join("prefs.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
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
