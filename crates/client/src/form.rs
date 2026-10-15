//! Multi-field forms: setup, restore, backup, duress password, wipe.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zeroize::Zeroizing;

use crate::password;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Setup,
    Restore,
    Backup,
    Duress,
    Wipe,
}

pub struct Form {
    pub kind: Kind,
    pub fields: Vec<Zeroizing<String>>,
    pub field: usize,
    pub error: String,
    /// A passphrase we generated, shown once so it can be written down.
    pub generated: Option<Zeroizing<String>>,
}

pub enum Action {
    Stay,
    Submit,
    Cancel,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Setup => " termchat setup ",
            Self::Restore => " restore from backup ",
            Self::Backup => " back up your identity ",
            Self::Duress => " duress password ",
            Self::Wipe => " wipe everything ",
        }
    }

    /// Label and whether the field is secret.
    pub fn labels(self) -> &'static [(&'static str, bool)] {
        match self {
            Self::Setup => &[
                ("Relay URL", false),
                ("Relay onion URL (optional, ws://….onion/)", false),
                ("Use Tor? y/n (hides your IP; needs the onion URL)", false),
                ("Nickname (the only thing others learn about you)", false),
                ("Password", true),
                ("Repeat password", true),
            ],
            Self::Restore => &[("Backup file", false)],
            Self::Backup => &[
                ("Save to", false),
                ("Backup password", true),
                ("Repeat", true),
            ],
            Self::Duress => &[
                ("Current password", true),
                ("Duress password (empty: turn it off)", true),
                ("Repeat", true),
            ],
            Self::Wipe => &[("Type WIPE to confirm", false)],
        }
    }

    pub fn info(self) -> &'static str {
        match self {
            Self::Setup => {
                "No names, phones or emails: just a nickname.\nCtrl+G: generate a passphrase   Ctrl+B: restore from a backup"
            }
            Self::Restore => {
                "Afterwards unlock with the backup's password. Chats start fresh;\ncontacts see that you restarted the conversation.   Esc: back"
            }
            Self::Backup => {
                "Your identity and contacts, no messages. Keep it OFF this device\n(USB stick, another computer). Ctrl+G: generate a password"
            }
            Self::Duress => {
                "Entering the duress password at unlock silently erases everything\nand shows the first-run screen. It does NOT help if someone copied\nthe disk first. The code is public, so they may know the feature.\nDestroying data after seizure can be a crime where you live."
            }
            Self::Wipe => {
                "Erases keys, contacts and history from this device. No undo.\nWithout a backup your identity is gone for good."
            }
        }
    }

    /// Which secret field Ctrl+G fills (with the next one as its repeat).
    fn generated_field(self) -> Option<usize> {
        match self {
            Self::Setup => Some(4),
            Self::Backup | Self::Duress => Some(1),
            Self::Restore | Self::Wipe => None,
        }
    }
}

impl Form {
    pub fn new(kind: Kind) -> Self {
        let mut fields: Vec<_> = kind.labels().iter().map(|_| Zeroizing::default()).collect();
        if kind == Kind::Setup {
            fields[2] = Zeroizing::new("y".into());
        }
        if kind == Kind::Backup {
            let default = std::env::home_dir()
                .unwrap_or_default()
                .join("termchat-backup");
            fields[0] = Zeroizing::new(default.display().to_string());
        }
        Self {
            kind,
            fields,
            field: 0,
            error: String::new(),
            generated: None,
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        let n = self.fields.len();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return Action::Cancel,
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % n,
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + n - 1) % n,
            KeyCode::Char('g') if ctrl => {
                if let Some(i) = self.kind.generated_field() {
                    let phrase = password::generate();
                    self.fields[i] = phrase.clone();
                    self.fields[i + 1] = phrase.clone();
                    self.generated = Some(phrase);
                }
            }
            KeyCode::Char(c) if !ctrl => self.fields[self.field].push(c),
            KeyCode::Backspace => {
                self.fields[self.field].pop();
            }
            KeyCode::Enter if self.field + 1 < n => self.field += 1,
            KeyCode::Enter => return Action::Submit,
            _ => {}
        }
        Action::Stay
    }

    /// Two password fields must match and be strong.
    pub fn new_password(&self, at: usize, avoid: &[&str]) -> Result<&str, String> {
        let (pw, repeat) = (&self.fields[at], &self.fields[at + 1]);
        if pw != repeat {
            return Err("passwords do not match".into());
        }
        if let Some(why) = password::weakness(pw, avoid) {
            return Err(why);
        }
        Ok(pw.as_str())
    }
}

/// `~/x` to the home folder; other paths as typed.
pub fn expand(path: &str) -> std::path::PathBuf {
    match path.trim().strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => path.trim().into(),
    }
}
