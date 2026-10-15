//! App data stored inside the encrypted state file, and writing that file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use termchat_proto::Ed25519Key;
use zeroize::Zeroizing;

/// Everything the TUI keeps besides the crypto state. No personal data:
/// only our own nickname and what we chose to call our contacts (rule 14).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    /// A backup of the identity exists; until then the status bar nags.
    pub backed_up: bool,
    pub relay_url: String,
    /// The relay's onion address, if it has one (ADR-004).
    pub onion_url: Option<String>,
    /// Connect only through Tor.
    pub tor: bool,
    pub nickname: String,
    pub next_client_id: u64,
    pub fallback_published: bool,
    /// When a fallback key was rotated out; forgotten a day after the relay TTL.
    pub old_fallback_since: Option<u64>,
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Contact {
    pub key: Ed25519Key,
    /// Local name; never sent anywhere.
    pub petname: Option<String>,
    /// Safety number compared out of band.
    pub verified: bool,
    /// Added from their invite; our `ContactRequest` is not sent yet.
    pub request_pending: bool,
    /// Disappearing timer for messages we send, seconds.
    pub expires_in: Option<u32>,
    /// Keep this chat in memory only, never on disk.
    pub no_history: bool,
    /// Their messages are dropped unread; we cannot write to them.
    pub blocked: bool,
    pub messages: Vec<Msg>,
    #[serde(skip)]
    pub unread: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub mine: bool,
    pub text: String,
    /// Unix seconds, local clock.
    pub at: u64,
    pub expires_at: Option<u64>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Waiting for a session with the contact.
    Queued,
    Sending(u64),
    /// Accepted by the relay.
    Sent,
    Failed(String),
    Received,
    /// A notice from the app, not from the contact.
    System,
}

impl Data {
    /// JSON for the state file, without chats marked "no history". Zeroized
    /// on drop: it holds the whole history in the clear.
    pub fn to_json(&self) -> Result<Zeroizing<String>, serde_json::Error> {
        let mut copy = self.clone();
        for c in copy.contacts.iter_mut().filter(|c| c.no_history) {
            c.messages.clear();
        }
        serde_json::to_string(&copy).map(Zeroizing::new)
    }

    /// After a restart, sends that were in flight have no transport to confirm them.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let mut data: Self = serde_json::from_str(json)?;
        for m in data.contacts.iter_mut().flat_map(|c| c.messages.iter_mut()) {
            if matches!(m.status, Status::Sending(_)) {
                m.status = Status::Failed("not confirmed before exit".into());
            }
        }
        Ok(data)
    }
}

/// `$TERMCHAT_HOME`, else `~/.termchat`.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("TERMCHAT_HOME") {
        return dir.into();
    }
    std::env::home_dir().unwrap_or_default().join(".termchat")
}

/// Temp file, fsync, rename: a crash leaves either the old or the new state.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    // The rename itself lives in the directory entry; flush that too.
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Crypto-erase (ADR-003): random bytes over the whole file, fsync, delete.
/// Without the header the body is noise. On SSDs old blocks may survive until
/// TRIM; see the ADR for what that means.
pub fn wipe(path: &Path) -> io::Result<()> {
    for p in [path.to_path_buf(), path.with_extension("tmp")] {
        let Ok(len) = fs::metadata(&p).map(|m| m.len()) else {
            continue;
        };
        let mut noise = vec![0u8; usize::try_from(len).unwrap_or(0)];
        getrandom::fill(&mut noise).map_err(|e| io::Error::other(e.to_string()))?;
        let mut file = OpenOptions::new().write(true).open(&p)?;
        file.write_all(&noise)?;
        file.sync_all()?;
        fs::remove_file(&p)?;
    }
    Ok(())
}

/// A warning if other users could read the state file or its folder.
pub fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let loose = |p: &Path| fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o077 != 0);
        let dir = path.parent()?;
        if loose(path) || loose(dir) {
            return Some(format!(
                "WARNING: others can read {}; run: chmod 700 {} && chmod 600 {}",
                dir.display(),
                dir.display(),
                path.display()
            ));
        }
    }
    let _ = path;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_is_private_and_wipe_removes_everything() {
        let dir = std::env::temp_dir().join(format!("termchat-test-{}", std::process::id()));
        let path = dir.join("state");
        write_atomic(&path, b"secret").unwrap();
        assert_eq!(permission_warning(&path), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(permission_warning(&path).is_some());
        }
        wipe(&path).unwrap();
        assert!(!path.exists() && !path.with_extension("tmp").exists());
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn no_history_chats_are_not_saved_and_inflight_sends_fail() {
        let mut data = Data::default();
        let msg = |status| Msg {
            mine: true,
            text: "x".into(),
            at: 0,
            expires_at: None,
            status,
        };
        let mut kept = Contact {
            key: [1; 32],
            ..Default::default()
        };
        kept.messages.push(msg(Status::Sending(1)));
        let mut secret = Contact {
            key: [2; 32],
            ..Default::default()
        };
        secret.no_history = true;
        secret.messages.push(msg(Status::Sent));
        data.contacts = vec![kept, secret];

        let back = Data::from_json(&data.to_json().unwrap()).unwrap();
        assert!(back.contacts[1].messages.is_empty());
        assert!(matches!(
            back.contacts[0].messages[0].status,
            Status::Failed(_)
        ));
    }
}
