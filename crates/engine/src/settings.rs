//! Interface settings: a plain file next to the state, so the lock screen can
//! use the chosen theme before the password is typed (ADR-008). Nothing in
//! here says anything about contacts.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
    /// Minutes without input before the app locks itself (ADR-021).
    pub lock_minutes: u64,
}

/// The lock times a user can pick, in minutes; no "never" (ADR-021).
pub const LOCK_CHOICES: [u64; 4] = [1, 5, 15, 30];

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: themes::THEMES[0].0.into(),
            mouse: true,
            sidebar: 28,
            sound: true,
            notify: false,
            desktop_theme: "termchat".into(),
            lock_minutes: 5,
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

    /// Idle time before the lock. The file is plain: a value edited outside
    /// the choices is held to 1-30 minutes.
    pub fn lock_after(&self) -> Duration {
        Duration::from_secs(self.lock_minutes.clamp(1, 30) * 60)
    }

    /// The next of `LOCK_CHOICES`, round.
    pub fn next_lock(&mut self) {
        let at = LOCK_CHOICES.iter().position(|m| *m == self.lock_minutes);
        self.lock_minutes = LOCK_CHOICES[at.map_or(1, |i| (i + 1) % LOCK_CHOICES.len())];
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        data::write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }
}

/// Time without input, for the idle lock (ADR-011, ADR-021). It counts on
/// both clocks and takes the longer: the monotonic one stops while the
/// computer sleeps, the wall clock can be set back.
#[derive(Clone, Copy)]
pub struct Idle {
    mono: Instant,
    wall: SystemTime,
}

impl Default for Idle {
    fn default() -> Self {
        Self {
            mono: Instant::now(),
            wall: SystemTime::now(),
        }
    }
}

impl Idle {
    pub fn elapsed(&self) -> Duration {
        let wall = self.wall.elapsed().unwrap_or_default();
        self.mono.elapsed().max(wall)
    }

    /// Input: count again from now, unless `limit` has already passed;
    /// `false` then, and the caller locks. The key that wakes the computer
    /// must not undo the lock that was due while it slept.
    pub fn touch(&mut self, limit: Duration) -> bool {
        let fresh = self.elapsed() < limit;
        if fresh {
            *self = Self::default();
        }
        fresh
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

    #[test]
    fn sleep_counts_and_late_input_does_not_undo_the_lock() {
        // The computer slept 10 minutes: only the wall clock moved.
        let mut idle = Idle {
            mono: Instant::now(),
            wall: SystemTime::now() - Duration::from_secs(600),
        };
        assert!(idle.elapsed() >= Duration::from_secs(600));
        assert!(!idle.touch(Duration::from_secs(300)));
        assert!(
            idle.elapsed() >= Duration::from_secs(600),
            "late input reset the time"
        );
        assert!(idle.touch(Duration::from_secs(900)));
        assert!(idle.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn lock_time_stays_within_the_choices() {
        let mut s = Settings::default();
        assert_eq!(s.lock_after(), Duration::from_secs(300));
        let seen: Vec<u64> = (0..4)
            .map(|_| {
                s.next_lock();
                s.lock_minutes
            })
            .collect();
        assert_eq!(seen, [15, 30, 1, 5]);
        s.lock_minutes = 0;
        assert_eq!(s.lock_after(), Duration::from_secs(60));
        s.lock_minutes = u64::MAX;
        assert_eq!(s.lock_after(), Duration::from_secs(30 * 60));
        s.next_lock();
        assert_eq!(s.lock_minutes, 5);
    }
}
