//! Interface settings: a plain file next to the state, so the lock screen can
//! use the chosen theme before the password is typed (ADR-008). Nothing in
//! here says anything about contacts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::data;
use crate::theme::{self, Palette};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    /// Clicks and the wheel; off gives the terminal's own text selection back.
    pub mouse: bool,
    /// Contact list width, its edge included.
    pub sidebar: u16,
    /// A system sound for new messages (ADR-008).
    pub sound: bool,
    /// A system notification for new messages, without name or text.
    pub notify: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: theme::THEMES[0].0.into(),
            mouse: true,
            sidebar: 28,
            sound: true,
            notify: false,
        }
    }
}

/// Lives beside the state file, and is wiped with it.
pub fn path(state: &Path) -> PathBuf {
    state.with_file_name("settings.json")
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

    pub fn palette(&self) -> &'static Palette {
        theme::by_name(&self.theme)
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
