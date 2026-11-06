//! Interface settings: a plain file next to the state, so the lock screen can
//! use the chosen theme before the password is typed (ADR-008). Nothing in
//! here says anything about contacts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{data, themes};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    /// TUI: clicks and the wheel; off gives the terminal's own text selection
    /// back.
    pub mouse: bool,
    /// TUI: contact list width, its edge included.
    pub sidebar: u16,
    /// A system sound for new messages (ADR-008).
    pub sound: bool,
    /// A system notification for new messages, without name or text.
    pub notify: bool,
    /// The desktop window's theme; it starts with the Android look (ADR-018).
    pub desktop_theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: themes::THEMES[0].0.into(),
            mouse: true,
            sidebar: 28,
            sound: true,
            notify: false,
            desktop_theme: "termchat".into(),
        }
    }
}

/// Lives beside the state file, and is wiped with it.
pub fn path(state: &Path) -> PathBuf {
    data::settings_path(state)
}

impl Settings {
    /// A missing or broken file means defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        data::write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_broken_file_is_default() {
        let dir = std::env::temp_dir().join(format!("termchat-settings-{}", std::process::id()));
        let path = path(&dir.join("state"));
        assert_eq!(Settings::load(&path), Settings::default());
        let s = Settings {
            theme: "nord".into(),
            ..Settings::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        std::fs::write(&path, b"{oops").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
