//! App data stored inside the encrypted state file, and writing that file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use termchat_core::store::file_id;
use termchat_proto::{Body, Ed25519Key, InviteCode};
use zeroize::{Zeroize, Zeroizing};

/// Everything the TUI keeps besides the crypto state. No personal data:
/// only our own nickname and what we chose to call our contacts (rule 14).
#[derive(Clone, Default, Serialize, Deserialize)]
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
    /// Requests we turned down; their messages are dropped unread (ADR-007).
    pub rejected: Vec<Ed25519Key>,
    /// The version whose "what's new" was opened; the status bar nags until then.
    pub news_seen: String,
    /// Where the relay wakes this account (UnifiedPush, ADR-012); told on every connect.
    pub push: Option<String>,
    /// Codes of our invites that still let a stranger in (ADR-013).
    pub invites: Invites,
    /// Incognito identities, one per contact (ADR-014).
    pub profiles: Vec<Profile>,
    /// Tor bridge lines; with any, Tor enters only through them (ADR-015).
    pub bridges: Vec<String>,
    /// The transport program, if not lyrebird next to termchat or on `PATH`.
    pub pt_program: Option<String>,
    /// Which Arti folder is current: a new one for each change of bridges.
    pub tor_generation: u32,
}

/// An identity of its own, used with one contact only (ADR-014).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Its Ed25519 key, which the contact and the relay know it by.
    pub key: Ed25519Key,
    /// What this contact sees as our name.
    pub nickname: String,
    /// `core::Client::export`, filled only while saving; the main state file seals it.
    pub state: String,
    /// Its own UnifiedPush endpoint: a shared one would link the identities.
    pub push: Option<String>,
    pub fallback_published: bool,
    pub old_fallback_since: Option<u64>,
    /// Its own `Send` numbering, from a random start: one counter for every
    /// identity would show the relay which keys belong together.
    pub next_client_id: u64,
    /// The code of its invite until someone uses it; then the profile is theirs.
    pub code: Option<InviteCode>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Invites {
    /// Unused one-time codes, oldest first.
    pub once: Vec<InviteCode>,
    pub public: Option<InviteCode>,
    /// Drop requests without a code. Off for accounts older than ADR-013,
    /// whose code-less invites are already out.
    pub codes_only: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Contact {
    pub key: Ed25519Key,
    /// Local name; never sent anywhere.
    pub petname: Option<String>,
    /// Safety number compared out of band.
    pub verified: bool,
    /// Added from their invite; our `ContactRequest` is not sent yet.
    pub request_pending: bool,
    /// The code from their invite, echoed in our request (ADR-013).
    #[serde(default)]
    pub invite_code: Option<InviteCode>,
    /// Our incognito identity for this contact; `None` is the main one (ADR-014).
    #[serde(default)]
    pub profile: Option<Ed25519Key>,
    /// Disappearing timer for messages we send, seconds.
    pub expires_in: Option<u32>,
    /// Keep this chat in memory only, never on disk.
    pub no_history: bool,
    /// Their messages are dropped unread; we cannot write to them.
    pub blocked: bool,
    /// Texts from a request not accepted yet, shown once it is (ADR-007).
    pub hidden: Vec<Msg>,
    /// Further texts from that request that were not kept.
    pub held_back: u32,
    pub messages: Vec<Msg>,
    pub unread: usize,
    /// When the contact was added; orders the list until there are messages.
    pub added_at: u64,
    /// They wiped their account and said goodbye; the chat is read-only (ADR-009).
    #[serde(default)]
    pub left: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Msg {
    pub mine: bool,
    pub text: String,
    /// Unix seconds, local clock.
    pub at: u64,
    pub expires_at: Option<u64>,
    pub status: Status,
    /// A document instead of a text; `text` holds its name (ADR-017).
    #[serde(default)]
    pub file: Option<FileMsg>,
}

/// A document in a chat (ADR-017).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct FileMsg {
    pub id: u32,
    pub name: String,
    pub size: u32,
    /// Chunks sent so far, or received so far.
    pub done: u16,
    /// The bytes: ours until the last chunk is sent, theirs until saved or
    /// discarded. Base64 in the state file.
    #[serde(with = "b64_bytes")]
    pub data: Option<Vec<u8>>,
}

impl FileMsg {
    pub fn complete(&self) -> bool {
        usize::from(self.done) == termchat_proto::file_chunks(self.size)
    }

    /// One line for the chat: name, size and, while it travels, progress.
    pub fn label(&self) -> String {
        let size = match self.size {
            n if n < 1024 => format!("{n} B"),
            n => format!("{} KB", n.div_ceil(1024)),
        };
        let total = termchat_proto::file_chunks(self.size);
        match self.complete() {
            true => format!("📄 {} · {size}", self.name),
            false => format!("📄 {} · {size} · {}/{total}", self.name, self.done),
        }
    }

    /// Chunk number `done`, the next one to send; `None` once all are out.
    pub fn next_chunk(&self, expires_in: Option<u32>) -> Option<Body> {
        let len = termchat_proto::chunk_len(self.size, self.done)?;
        let start = usize::from(self.done) * termchat_proto::FILE_CHUNK;
        Some(Body::File {
            id: self.id,
            name: self.name.clone(),
            size: self.size,
            index: self.done,
            expires_in,
            data: self.data.as_ref()?.get(start..start + len)?.to_vec(),
        })
    }
}

/// ADR-017: received files waiting to be saved or discarded, in total.
pub const MAX_UNSAVED: usize = 8 << 20;

impl Data {
    /// One received chunk for contact `i` (ADR-017): in order, one file per
    /// contact at a time. `true` once the file is whole.
    pub fn take_chunk(&mut self, i: usize, chunk: Body, at: u64) -> bool {
        let Body::File {
            id,
            name,
            size,
            index,
            expires_in,
            data,
        } = chunk
        else {
            return false;
        };
        let open = |m: &Msg| {
            !m.mine
                && m.file
                    .as_ref()
                    .is_some_and(|f| f.data.is_some() && !f.complete())
        };
        let note = |text: String| Msg {
            mine: false,
            text,
            at,
            expires_at: None,
            status: Status::System,
            file: None,
        };
        let Some(c) = self.contacts.get_mut(i) else {
            return false;
        };
        if let Some(m) = c.messages.iter().rposition(open) {
            if let Some(f) = c.messages[m].file.as_mut()
                && (f.id, &f.name, f.size, f.done) == (id, &name, size, index)
                && let Some(bytes) = f.data.as_mut()
            {
                bytes.extend_from_slice(&data);
                f.done += 1;
                return f.complete();
            }
            let broken = c.messages.remove(m).text;
            c.messages
                .push(note(format!("{broken} did not arrive whole")));
        }
        if index != 0 {
            return false;
        }
        let unsaved: usize = (self.contacts.iter())
            .flat_map(|c| &c.messages)
            .filter(|m| !m.mine)
            .filter_map(|m| m.file.as_ref()?.data.as_ref())
            .map(Vec::len)
            .sum();
        let c = &mut self.contacts[i];
        if unsaved + size as usize > MAX_UNSAVED {
            c.messages.push(note(format!(
                "{name} was dropped: too many unsaved files, save or discard some"
            )));
            return false;
        }
        let file = FileMsg {
            id,
            name: name.clone(),
            size,
            done: 1,
            data: Some(data),
        };
        let whole = file.complete();
        c.messages.push(Msg {
            mine: false,
            text: name,
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Received,
            file: Some(file),
        });
        whole
    }
}

/// Creates `name` in `dir`, or `name (1)`, `name (2)`... if taken, mode 0600,
/// and writes `bytes` (ADR-017). Never overwrites anything.
pub fn save_new(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    for n in 0..100 {
        let path = dir.join(match n {
            0 => name.to_string(),
            n => format!("{stem} ({n}){ext}"),
        });
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut file = match opts.open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        return Ok(path);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many files with this name",
    ))
}

mod b64_bytes {
    use super::{B64, Engine};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_some(&B64.encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|t| B64.decode(t).map_err(D::Error::custom))
            .transpose()
    }
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

impl Invites {
    /// Whether a stranger's request may in; spends a one-time code.
    pub fn admit(&mut self, body: &Body) -> bool {
        let code = match body {
            Body::ContactRequestV2 { code, .. } => code,
            _ => return !self.codes_only,
        };
        // Every code is compared, so the time does not tell which one matched.
        let mut hit = None;
        for (i, c) in self.once.iter().enumerate() {
            if bool::from(c.ct_eq(code)) {
                hit = Some(i);
            }
        }
        let public = self.public.is_some_and(|c| bool::from(c.ct_eq(code)));
        if let Some(i) = hit {
            self.once.remove(i);
        }
        hit.is_some() || public
    }
}

impl Data {
    /// JSON for the state file, without chats marked "no history". Zeroized
    /// on drop: it holds the whole history in the clear.
    /// `states` are the incognito profiles' exported keys, in order.
    pub fn to_json(
        &self,
        states: &[Zeroizing<String>],
    ) -> Result<Zeroizing<String>, serde_json::Error> {
        let mut copy = self.clone();
        for c in copy.contacts.iter_mut().filter(|c| c.no_history) {
            c.messages.clear();
            c.hidden.clear();
        }
        for (p, state) in copy.profiles.iter_mut().zip(states) {
            p.state.clone_from(state);
        }
        let json = serde_json::to_string(&copy).map(Zeroizing::new);
        copy.profiles.iter_mut().for_each(|p| p.state.zeroize());
        json
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

/// `~/x` to the home folder; other paths as typed.
pub fn expand(path: &str) -> PathBuf {
    match path.trim().strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => path.trim().into(),
    }
}

/// Temp file, fsync, rename: a crash leaves either the old or the new state.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = create_private(&tmp)?;
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

/// Opens `path` for writing with mode 0600, creating it and its folder
/// (0700) if needed.
fn create_private(path: &Path) -> io::Result<fs::File> {
    if let Some(dir) = path.parent() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir)?;
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)
}

/// Takes the lock that keeps a second client off the same state: two would
/// each advance the Olm sessions and overwrite each other (ADR-018). Held
/// until the file is dropped or the process ends. `None`: another client has
/// it. The empty `lock` file stays, so a wiped folder still looks like a
/// fresh install that has been started once.
pub fn lock(state: &Path) -> io::Result<Option<fs::File>> {
    let file = create_private(&state.with_file_name("lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

impl Contact {
    /// Latest activity, for "most recent first" in the list.
    pub fn last_at(&self) -> u64 {
        let last = |m: &[Msg]| m.last().map_or(0, |m| m.at);
        self.added_at
            .max(last(&self.messages))
            .max(last(&self.hidden))
    }
}

/// Interface settings (theme and the like): plain, so the lock screen can use
/// them before the password (ADR-008). Lives beside the state, wiped with it.
pub fn settings_path(state: &Path) -> PathBuf {
    state.with_file_name("settings.json")
}

/// Tor state and directory cache (ADR-004); shows that termchat was used
/// over Tor, and holds the bridge lines in use in the clear (ADR-015).
pub fn tor_dir(state: &Path) -> PathBuf {
    state.with_file_name("arti")
}

/// Arti's folder for one set of bridges: a client that is still shutting
/// down keeps its own (ADR-015).
pub fn tor_dir_for(state: &Path, generation: u32) -> PathBuf {
    tor_dir(state).join(format!("g{generation}"))
}

/// Erases every Arti folder but the current one; best effort, as an old
/// client may still be closing.
pub fn prune_tor_dirs(state: &Path, generation: u32) {
    let keep = tor_dir_for(state, generation);
    let Ok(entries) = fs::read_dir(tor_dir(state)) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p != keep {
            let _ = if p.is_dir() {
                fs::remove_dir_all(&p)
            } else {
                fs::remove_file(&p)
            };
        }
    }
}

/// Crypto-erase (ADR-003): random bytes over the whole file, fsync, delete.
/// The interface settings and the Tor directory go too, so nothing is left
/// behind (ADR-004, ADR-008). Without the header the body is noise. On SSDs
/// old blocks may survive until TRIM; see the ADR for what that means.
/// Backups still at the path they were written to or restored from go as well.
pub fn wipe(path: &Path) -> io::Result<()> {
    let settings = settings_path(path);
    for p in [path.to_path_buf(), path.with_extension("tmp"), settings] {
        shred(&p)?;
    }
    let list = backups(path);
    for line in fs::read_to_string(&list).unwrap_or_default().lines() {
        let Some((id, p)) = line.split_once(' ') else {
            continue;
        };
        // Only the file we wrote: a planted line must not erase anything else.
        // A read-only or missing stick must not stop the wipe either.
        if fs::read(p)
            .ok()
            .and_then(|b| file_id(&b))
            .map(|f| B64.encode(f))
            == Some(id.into())
        {
            let _ = shred(Path::new(p));
        }
    }
    shred(&list)?;
    remove_tor_dir(path)
}

fn shred(p: &Path) -> io::Result<()> {
    let Ok(len) = fs::metadata(p).map(|m| m.len()) else {
        return Ok(());
    };
    let mut noise = vec![0u8; usize::try_from(len).unwrap_or(0)];
    getrandom::fill(&mut noise).map_err(|e| io::Error::other(e.to_string()))?;
    let mut file = OpenOptions::new().write(true).open(p)?;
    file.write_all(&noise)?;
    file.sync_all()?;
    fs::remove_file(p)
}

/// Where backups were written or restored from, for `wipe` (ADR-003). Plain,
/// because the duress password cannot open the state file: `id path` per line.
fn backups(state: &Path) -> PathBuf {
    state.with_file_name("backups")
}

/// Notes `backup` (a file with `bytes`) in the list `wipe` reads.
pub fn remember_backup(state: &Path, backup: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(id) = file_id(bytes) else {
        return Ok(());
    };
    let backup = fs::canonicalize(backup)?;
    let line = format!("{} {}\n", B64.encode(id), backup.display());
    let mut list = fs::read_to_string(backups(state)).unwrap_or_default();
    if !list.contains(&line) {
        list.push_str(&line);
        write_atomic(&backups(state), list.as_bytes())?;
    }
    Ok(())
}

/// Also run on a first-run screen: Arti may still have been flushing its state
/// while the wipe ran, and a fresh install has nothing to keep there anyway.
pub fn remove_tor_dir(state: &Path) -> io::Result<()> {
    match fs::remove_dir_all(tor_dir(state)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// A warning if other users could read the state file or its folder.
/// Not on Android: each app runs as its own user, so its folder is private anyway.
pub fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(all(unix, not(target_os = "android")))]
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

    fn outgoing(bytes: Vec<u8>) -> FileMsg {
        FileMsg {
            id: 7,
            name: "report.pdf".into(),
            size: bytes.len() as u32,
            done: 0,
            data: Some(bytes),
        }
    }

    fn chunks(mut f: FileMsg) -> Vec<Body> {
        let mut out = Vec::new();
        while let Some(chunk) = f.next_chunk(Some(60)) {
            out.push(chunk);
            f.done += 1;
        }
        out
    }

    fn one_contact() -> Data {
        let mut data = Data::default();
        data.contacts.push(Contact::default());
        data
    }

    #[test]
    fn file_goes_through_in_chunks_and_survives_the_state_file() {
        let bytes: Vec<u8> = (0..40_000u32).map(|n| n as u8).collect();
        let sent = chunks(outgoing(bytes.clone()));
        assert_eq!(sent.len(), 3);
        let mut data = one_contact();
        let whole: Vec<bool> = sent
            .into_iter()
            .map(|c| data.take_chunk(0, c, 100))
            .collect();
        assert_eq!(whole, [false, false, true]);
        let data = Data::from_json(&data.to_json(&[]).unwrap()).unwrap();
        let m = &data.contacts[0].messages[0];
        assert_eq!((m.text.as_str(), m.expires_at), ("report.pdf", Some(160)));
        assert_eq!(m.file.as_ref().unwrap().data.as_deref(), Some(&bytes[..]));
    }

    #[test]
    fn chunks_out_of_order_break_the_file() {
        let sent = chunks(outgoing(vec![1; 40_000]));
        let mut data = one_contact();
        assert!(!data.take_chunk(0, sent[1].clone(), 0));
        assert!(data.contacts[0].messages.is_empty());
        assert!(!data.take_chunk(0, sent[0].clone(), 0));
        assert!(!data.take_chunk(0, sent[2].clone(), 0));
        let msgs = &data.contacts[0].messages;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "report.pdf did not arrive whole");
        // A new file after a broken one starts clean.
        assert!(data.take_chunk(0, chunks(outgoing(vec![2; 10]))[0].clone(), 0));
    }

    #[test]
    fn unsaved_files_are_capped() {
        let big = |id| {
            let mut f = outgoing(vec![0; termchat_proto::MAX_FILE_BYTES as usize]);
            f.id = id;
            chunks(f)
        };
        let mut data = one_contact();
        for id in 0..8 {
            assert!(
                big(id)
                    .into_iter()
                    .map(|c| data.take_chunk(0, c, 0))
                    .last()
                    .unwrap()
            );
        }
        assert!(!data.take_chunk(0, big(9)[0].clone(), 0));
        let last = data.contacts[0].messages.last().unwrap();
        assert_eq!(last.status, Status::System);
        assert!(last.text.contains("too many unsaved files"));
        // Discarding one makes room again.
        data.contacts[0].messages[0].file.as_mut().unwrap().data = None;
        assert!(!data.take_chunk(0, big(10)[0].clone(), 0));
        assert_eq!(
            data.contacts[0].messages.last().unwrap().status,
            Status::Received
        );
    }

    #[test]
    fn saving_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("termchat-save-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let first = save_new(&dir, "a.pdf", b"one").unwrap();
        let second = save_new(&dir, "a.pdf", b"two").unwrap();
        let bare = save_new(&dir, ".env", b"x").unwrap();
        let bare2 = save_new(&dir, ".env", b"y").unwrap();
        assert_eq!(first, dir.join("a.pdf"));
        assert_eq!(second, dir.join("a (1).pdf"));
        assert_eq!(bare2, dir.join(".env (1)"));
        assert_eq!(fs::read(&first).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&second).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        for p in [first, second, bare, bare2] {
            fs::remove_file(p).unwrap();
        }
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn invite_codes_admit_once_public_until_renewed_and_old_by_setting() {
        let identity = termchat_proto::Identity {
            ed25519: [1; 32],
            curve25519: [2; 32],
            signature: [3; 64],
        };
        let v2 = |code| Body::ContactRequestV2 {
            identity: identity.clone(),
            nickname: "x".into(),
            note: String::new(),
            code,
        };
        let old = Body::ContactRequest {
            identity: identity.clone(),
            nickname: "x".into(),
            note: String::new(),
        };
        let mut inv = Invites {
            once: vec![[1; 16], [2; 16]],
            public: Some([7; 16]),
            codes_only: true,
        };
        assert!(inv.admit(&v2([2; 16])));
        assert!(!inv.admit(&v2([2; 16])), "a one-time code works once");
        assert_eq!(inv.once, vec![[1; 16]]);
        assert!(inv.admit(&v2([7; 16])) && inv.admit(&v2([7; 16])));
        assert!(!inv.admit(&v2([9; 16])));
        assert!(!inv.admit(&old));
        inv.codes_only = false;
        assert!(inv.admit(&old));
        inv.public = None;
        assert!(
            !inv.admit(&v2([7; 16])),
            "a renewed public code stops the old one"
        );
        // Accounts from before ADR-013 have no field: old invites keep working.
        let data: Data = serde_json::from_str("{}").unwrap();
        assert!(!data.invites.codes_only);
    }

    #[test]
    fn write_is_private_and_wipe_removes_everything() {
        let dir = std::env::temp_dir().join(format!("termchat-test-{}", std::process::id()));
        let path = dir.join("state");
        write_atomic(&path, b"secret").unwrap();
        assert_eq!(permission_warning(&path), None);
        #[cfg(all(unix, not(target_os = "android")))]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(permission_warning(&path).is_some());
        }
        write_atomic(&settings_path(&path), b"{}").unwrap();
        write_atomic(&tor_dir(&path).join("state").join("guards"), b"[]").unwrap();
        // A known backup goes; one replaced by another file at its path stays.
        let header = |b: u8| vec![b; 200];
        let (ours, theirs) = (dir.join("ours"), dir.join("theirs"));
        for (p, b) in [(&ours, 1), (&theirs, 2)] {
            write_atomic(p, &header(b)).unwrap();
            remember_backup(&path, p, &header(b)).unwrap();
        }
        write_atomic(&theirs, &header(3)).unwrap();
        wipe(&path).unwrap();
        assert!(!ours.exists() && theirs.exists() && !backups(&path).exists());
        fs::remove_file(&theirs).unwrap();
        assert!(!path.exists() && !path.with_extension("tmp").exists());
        assert!(!settings_path(&path).exists());
        assert!(!tor_dir(&path).exists());
        // Nothing to remove is not an error.
        remove_tor_dir(&path).unwrap();
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn second_lock_waits_for_the_first() {
        let dir = std::env::temp_dir().join(format!("termchat-lock-{}", std::process::id()));
        let path = dir.join("state");
        let first = lock(&path).unwrap();
        assert!(first.is_some());
        assert!(lock(&path).unwrap().is_none(), "a second client is refused");
        drop(first);
        assert!(lock(&path).unwrap().is_some(), "free again once dropped");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_at_orders_most_recent_first() {
        let msg = |at| Msg {
            mine: false,
            text: String::new(),
            at,
            expires_at: None,
            status: Status::Received,
            file: None,
        };
        let new = Contact {
            key: [1; 32],
            added_at: 50,
            ..Default::default()
        };
        let mut old = Contact {
            key: [2; 32],
            added_at: 10,
            ..Default::default()
        };
        old.messages.push(msg(20));
        let mut talking = Contact {
            key: [3; 32],
            ..Default::default()
        };
        talking.hidden.push(msg(90));
        let mut list = [old, new, talking];
        list.sort_by_key(|c| std::cmp::Reverse(c.last_at()));
        let keys: Vec<u8> = list.iter().map(|c| c.key[0]).collect();
        assert_eq!(keys, [3, 1, 2]);
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
            file: None,
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

        let back = Data::from_json(&data.to_json(&[]).unwrap()).unwrap();
        assert!(back.contacts[1].messages.is_empty());
        assert!(matches!(
            back.contacts[0].messages[0].status,
            Status::Failed(_)
        ));
    }
}
