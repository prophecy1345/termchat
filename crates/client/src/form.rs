//! Multi-field forms: setup, restore, backup, duress password, wipe.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zeroize::Zeroizing;

use crate::input::Input;
use crate::password;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Setup,
    Restore,
    Backup,
    Duress,
    Password,
    Wipe,
}

pub use termchat_engine::engine::{DEFAULT_ONION, DEFAULT_RELAY};

pub struct Form {
    pub kind: Kind,
    pub fields: Vec<Input>,
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
            Self::Password => " change password ",
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
            Self::Password => &[
                ("Current password", true),
                ("New password", true),
                ("Repeat", true),
            ],
            Self::Wipe => &[("Type WIPE or MOVED", false)],
        }
    }

    pub fn info(self) -> &'static str {
        match self {
            Self::Setup => {
                "No names, phones or emails: just a nickname. The relay below is the\nproject's public one; keep it or type your own.\nCtrl+G: generate a passphrase   Ctrl+B: restore from a backup"
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
            Self::Password => {
                "The data stays as it is; only the key slot changes.\nIf you set a duress password, keep the new one far from it:\na typo that hits the duress password erases everything.   Ctrl+G: generate"
            }
            Self::Wipe => {
                "Erases keys, contacts and history from this device. No undo.\nWIPE deletes the account: contacts are told you left and the\nrelay forgets your key. MOVED (restored on another device):\nerases only this device and tells nobody."
            }
        }
    }

    /// Which secret field Ctrl+G fills (with the next one as its repeat).
    fn generated_field(self) -> Option<usize> {
        match self {
            Self::Setup => Some(4),
            Self::Backup | Self::Duress | Self::Password => Some(1),
            Self::Restore | Self::Wipe => None,
        }
    }
}

impl Form {
    pub fn new(kind: Kind) -> Self {
        let mut fields: Vec<Input> = kind.labels().iter().map(|_| Input::default()).collect();
        if kind == Kind::Setup {
            fields[0] = DEFAULT_RELAY.into();
            fields[1] = DEFAULT_ONION.into();
            fields[2] = "y".into();
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
                    self.fields[i] = phrase.as_str().into();
                    self.fields[i + 1] = phrase.as_str().into();
                    self.generated = Some(phrase);
                }
            }
            KeyCode::Enter if self.field + 1 < n => self.field += 1,
            KeyCode::Enter => return Action::Submit,
            _ => {
                self.fields[self.field].on_key(k);
            }
        }
        Action::Stay
    }

    pub fn paste(&mut self, text: &str) {
        self.fields[self.field].insert(text);
    }

    /// A new password and its repeat must match; the engine judges its strength.
    pub fn new_password(&self, at: usize) -> Result<&str, String> {
        let (pw, repeat) = (&self.fields[at], &self.fields[at + 1]);
        if pw != repeat {
            return Err("passwords do not match".into());
        }
        Ok(pw)
    }
}

/// `~/x` to the home folder; other paths as typed.
pub fn expand(path: &str) -> std::path::PathBuf {
    match path.trim().strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => path.trim().into(),
    }
}
