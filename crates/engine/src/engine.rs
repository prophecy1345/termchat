//! An unlocked account: the state, the relay link and everything that changes
//! them. The interface calls methods and drains `happened` afterwards.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use termchat_core::proto::{
    self, Body, ClientFrame, Ed25519Key, ErrorCode, Invite, InviteCode, ServerFrame, check_nickname,
};
use termchat_core::{
    Client, Error as CoreError, FileKey, KdfParams, create_file_key, set_duress, store,
};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_tungstenite::tungstenite::http::Uri;
use zeroize::Zeroize;

use crate::data::{self, Contact, Data, Msg, Status};
use crate::{Bridges, Dial, Error, Event, Relay, parse_version, password};
use subtle::ConstantTimeEq;

/// How long a wipe waits for the relay to take the goodbyes (ADR-009).
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);
const NOT_REMOVED: &str =
    "The relay did not confirm removing your key; it forgets idle keys after 90 days.";
/// ADR-007: what a not-yet-accepted request may leave behind.
const MAX_HIDDEN: usize = 5;
const MAX_REJECTED: usize = 1000;
/// ADR-003: a typo in the real password must never hit the duress one.
pub const DURESS_DISTANCE: usize = 4;
/// The project's public relay, prefilled on first run; any other relay can be
/// typed over it (docs/self-host.md).
pub const DEFAULT_RELAY: &str = "wss://relay.termchat.app/";
pub const DEFAULT_ONION: &str =
    "ws://rgisnh6oixs4usvi544poxwzxstbxls74x4mtzlwl4a55uuncrpolpad.onion/";

/// The first relay version that knows `SetPush` (ADR-012); older ones close
/// the connection on it.
const PUSH_SINCE: (u32, u32, u32) = (0, 1, 13);

/// Disappearing timer steps, in the order a button cycles them.
pub const TIMERS: [Option<u32>; 5] = [None, Some(300), Some(3600), Some(86_400), Some(604_800)];

pub fn next_timer(now: Option<u32>) -> Option<u32> {
    let at = TIMERS.iter().position(|t| *t == now).unwrap_or(0);
    TIMERS[(at + 1) % TIMERS.len()]
}

/// One-time codes kept unused; older ones stop working (ADR-013).
const MAX_ONE_TIME_CODES: usize = 20;

fn new_code() -> InviteCode {
    let mut code = [0; 16];
    // getrandom fails only on a broken OS; an all-zero code would still be
    // one shared, guessable code, so refuse to go on.
    if getrandom::fill(&mut code).is_err() {
        std::process::abort();
    }
    code
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the interface should show after a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Happened {
    /// A new message (`request: false`) or contact request to point out.
    Alert { key: Ed25519Key, request: bool },
    /// One line for the status bar: a problem, or the result of an action.
    Note(String),
    /// Worth a window of its own.
    Notice(String),
    /// Everything is erased; show the first-run screen with this note.
    Wiped(String),
    /// A wipe could not tell the relay: the reason, then ask "wipe anyway?".
    WipeAnyway(String),
}

/// Everything the first-run form asks.
pub struct Setup<'a> {
    pub relay_url: &'a str,
    pub onion_url: Option<&'a str>,
    pub tor: bool,
    pub nickname: &'a str,
    pub password: &'a str,
}

pub enum Unlocked {
    Open(Box<Engine>),
    /// The duress password was entered and the data is gone.
    Wiped,
}

pub struct Engine {
    /// The main identity.
    pub core: Arc<Mutex<Client>>,
    key: FileKey,
    pub data: Data,
    /// Slot 0 is the main identity, slot `n` is `data.profiles[n - 1]` (ADR-014).
    lives: Vec<Live>,
    /// Incognito identities asked to unregister, until the relay confirms.
    retiring: Vec<(Ed25519Key, Live, Instant)>,
    /// Every identity's relay events, tagged with its key.
    pub events: Receiver<RelayEvent>,
    events_tx: Sender<RelayEvent>,
    /// The main identity is connected.
    pub connected: bool,
    /// A newer version the relay runs (ADR-010).
    pub update: Option<String>,
    /// The relay knows `SetPush` (ADR-012).
    push_ok: bool,
    /// The chat on screen: its messages do not count as unread.
    pub open: Option<Ed25519Key>,
    pub happened: Vec<Happened>,
    path: PathBuf,
    leaving: Option<Leaving>,
    /// Erased: nothing may write the state file again.
    gone: bool,
}

/// A relay event and the key of our identity it is for (ADR-014).
pub type RelayEvent = (Ed25519Key, Event);

/// One identity's relay link. Dropping it closes the link.
struct Live {
    core: Arc<Mutex<Client>>,
    relay: Relay,
    connected: bool,
    forward: tokio::task::AbortHandle,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.forward.abort();
    }
}

/// How long an unregistering incognito identity may take (ADR-014).
const RETIRE_TIMEOUT: Duration = Duration::from_secs(60);
/// Incognito identities whose invite nobody used yet.
const MAX_WAITING_PROFILES: usize = 5;
const MISSING_PROFILE: &str = "this contact's incognito identity is missing";

/// Goodbyes sent and not yet accepted, then `Unregister` (ADR-009).
struct Leaving {
    waiting: Vec<u64>,
    until: Instant,
    unregistering: bool,
    /// Identities the relay has not yet confirmed forgetting (ADR-014).
    registered: Vec<Ed25519Key>,
}

/// Plain `ws://` would show the relay traffic (and so who we talk to) to the
/// network; it is only fine where TLS adds nothing: the same machine or an
/// onion service, which authenticates and encrypts on its own.
pub fn relay_url_ok(url: &str) -> bool {
    let Some(host) = ws_host(url) else {
        return false;
    };
    url.starts_with("wss://")
        || matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
        || host.ends_with(".onion")
}

/// The host the transport will actually connect to. Parsed the same way as in
/// `Relay::connect`, so userinfo, a query or a fragment cannot smuggle a
/// different host past the checks above.
fn ws_host(url: &str) -> Option<String> {
    let uri: Uri = url.parse().ok()?;
    if !matches!(uri.scheme_str(), Some("ws" | "wss")) {
        return None;
    }
    let host = uri.host()?.trim_start_matches('[').trim_end_matches(']');
    Some(host.to_owned())
}

/// Text from a peer or the network, safe to show on one line: no control
/// characters and no bidi overrides that could reorder what is around it.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|&c| {
            !c.is_control()
                && !matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

/// A message body, safe to show: like [`clean`], but line breaks stay.
pub fn clean_text(s: &str) -> String {
    s.split('\n').map(clean).collect::<Vec<_>>().join("\n")
}

fn why(code: ErrorCode) -> String {
    match code {
        ErrorCode::UnknownRecipient => "contact is no longer on the relay".into(),
        code => format!("{code:?}"),
    }
}

/// A backup is a state file without sessions or messages: restoring is a copy.
pub fn restore(state: &Path, backup: &Path) -> Result<(), String> {
    let bytes = std::fs::read(backup).map_err(|e| format!("cannot read backup: {e}"))?;
    data::write_atomic(state, &bytes).map_err(|e| e.to_string())?;
    // ADR-003: the copy brought onto this device is erased with it.
    data::remember_backup(state, backup, &bytes).map_err(|e| e.to_string())
}

/// Starts one identity's relay link: through Tor only if Tor is on (ADR-004).
/// Its events go to `tx` tagged with its key, after `delay` (ADR-014).
fn start(
    core: Arc<Mutex<Client>>,
    data: &Data,
    path: &Path,
    tx: &Sender<RelayEvent>,
    delay: Duration,
    incognito: bool,
) -> Result<Live, String> {
    let identity = core
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .identity();
    let me = identity.ed25519;
    let signer = Arc::clone(&core);
    let sign = Box::new(move |host: &str, nonce: &[u8; 32]| {
        signer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .auth_signature(host, nonce)
    });
    let (url, dial) = match (&data.onion_url, data.tor) {
        (Some(onion), true) => {
            let bridges = Bridges {
                lines: data.bridges.clone(),
                program: pt_program(data),
            };
            let dir = data::tor_dir_for(path, data.tor_generation);
            (onion.as_str(), Dial::Tor(dir, bridges))
        }
        (None, true) => return Err("Tor is on but the relay has no onion URL".into()),
        (_, false) => (data.relay_url.as_str(), Dial::Direct),
    };
    let (relay, mut rx) = Relay::connect_after(url, identity, sign, dial, delay, incognito)
        .map_err(|e| match e {
            Error::NoTor => "this build has no Tor support".to_string(),
            _ => format!("bad relay URL: {url}"),
        })?;
    // Stops with the engine (its receiver) or with this `Live` (abort); either
    // way the link's own receiver goes, and the link lets go of the keys.
    let tx = tx.clone();
    let forward = tokio::spawn(async move {
        loop {
            tokio::select! {
                ev = rx.recv() => {
                    let Some(ev) = ev else { return };
                    if tx.send((me, ev)).await.is_err() {
                        return;
                    }
                }
                _ = tx.closed() => return,
            }
        }
    })
    .abort_handle();
    Ok(Live {
        core,
        relay,
        connected: false,
        forward,
    })
}

/// Where the bridge transport program is (ADR-015): the setting, an app's
/// own copy (Android: [`set_pt_program`]), next to this program, or `PATH`.
pub fn pt_program(data: &Data) -> Option<PathBuf> {
    if let Some(p) = &data.pt_program {
        return Some(p.into());
    }
    if let Some(p) = PT_PROGRAM.get() {
        return Some(p.clone());
    }
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe()
        .ok()?
        .with_file_name(exe("lyrebird"));
    if beside.is_file() {
        return Some(beside);
    }
    let path = std::env::var_os("PATH")?;
    ["lyrebird", "obfs4proxy"].iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(exe(name)))
            .find(|p| p.is_file())
    })
}

static PT_PROGRAM: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The app's own transport program, e.g. in Android's native library folder.
pub fn set_pt_program(path: PathBuf) {
    let _ = PT_PROGRAM.set(path);
}

/// 0-30 s, so a device's identities do not connect in the same second (ADR-014).
fn spread() -> Duration {
    Duration::from_millis(u64::from(getrandom::u32().unwrap_or(0) % 30_000))
}

impl Engine {
    /// A new identity, saved to `path`.
    pub fn setup(path: &Path, s: &Setup) -> Result<Box<Self>, String> {
        if !relay_url_ok(s.relay_url) {
            return Err(
                "relay URL must start with wss:// (ws:// only for localhost or .onion)".into(),
            );
        }
        if s.onion_url.is_some_and(|o| {
            !(o.starts_with("ws://") && ws_host(o).is_some_and(|h| h.ends_with(".onion")))
        }) {
            return Err("onion URL looks like ws://xxxx.onion/".into());
        }
        if s.tor && s.onion_url.is_none() {
            return Err("Tor needs the relay's onion URL (or answer n)".into());
        }
        if s.tor && !cfg!(feature = "tor") {
            return Err("this build has no Tor support; answer n".into());
        }
        check_nickname(s.nickname).map_err(|_| "nickname: 1-64 bytes, no control characters")?;
        if let Some(why) = password::weakness(s.password, &[s.nickname]) {
            return Err(why);
        }
        let key = create_file_key(s.password.as_bytes(), KdfParams::DEFAULT)
            .map_err(|e| e.to_string())?;
        let data = Data {
            relay_url: s.relay_url.into(),
            onion_url: s.onion_url.map(Into::into),
            tor: s.tor,
            nickname: s.nickname.into(),
            news_seen: env!("CARGO_PKG_VERSION").into(),
            invites: data::Invites {
                codes_only: true,
                ..Default::default()
            },
            ..Data::default()
        };
        let mut engine = Self::open(Client::default(), key, data, path.to_owned())?;
        if !engine.persist() {
            return Err(engine.take_note());
        }
        Ok(engine)
    }

    /// Opens the state file. The duress password erases it first (ADR-003).
    pub fn unlock(path: &Path, password: &[u8]) -> Result<Unlocked, String> {
        // Argon2 blocks the caller for ~1 s.
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read state: {e}"))?;
        match Client::load(password, &bytes) {
            // Erase first, say nothing.
            Err(CoreError::Duress) => {
                // If shredding fails (read-only, locked file), still cut the
                // state to nothing: the screen must look fresh either way
                // (ADR-003), so the data must not survive behind it.
                if data::wipe(path).is_err() {
                    let cut = std::fs::OpenOptions::new().write(true).open(path);
                    let _ = cut.and_then(|f| f.set_len(0).and_then(|()| f.sync_all()));
                    let _ = std::fs::remove_file(path);
                }
                Ok(Unlocked::Wiped)
            }
            Err(CoreError::WrongPassword) => Err("wrong password".into()),
            Err(e) => Err(e.to_string()),
            Ok((core, key, app)) => {
                let data = Data::from_json(&app).map_err(|e| e.to_string())?;
                Self::open(core, key, data, path.to_owned()).map(Unlocked::Open)
            }
        }
    }

    fn open(
        core: Client,
        key: FileKey,
        mut data: Data,
        path: PathBuf,
    ) -> Result<Box<Self>, String> {
        let core = Arc::new(Mutex::new(core));
        let (events_tx, events) = mpsc::channel(256);
        let mut lives = vec![start(
            Arc::clone(&core),
            &data,
            &path,
            &events_tx,
            Duration::ZERO,
            false,
        )?];
        let mut cores = Vec::new();
        for p in &mut data.profiles {
            cores.push(Client::import(&p.state).map_err(|e| e.to_string())?);
            p.state.zeroize();
        }
        for c in cores {
            let c = Arc::new(Mutex::new(c));
            lives.push(start(c, &data, &path, &events_tx, spread(), true)?);
        }
        // Folders of earlier bridges, and of versions before ADR-015.
        data::prune_tor_dirs(&path, data.tor_generation);
        let happened = data::permission_warning(&path)
            .map(Happened::Notice)
            .into_iter()
            .collect();
        Ok(Box::new(Self {
            core,
            key,
            data,
            lives,
            retiring: Vec::new(),
            events,
            events_tx,
            connected: false,
            update: None,
            push_ok: false,
            open: None,
            happened,
            path,
            leaving: None,
            gone: false,
        }))
    }

    /// The main identity's state.
    pub fn core(&self) -> MutexGuard<'_, Client> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn core_at(&self, slot: usize) -> MutexGuard<'_, Client> {
        self.lives[slot]
            .core
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn me(&self) -> Ed25519Key {
        self.core().identity().ed25519
    }

    /// Our key as this contact knows it (ADR-014).
    pub fn me_for(&self, c: &Contact) -> Ed25519Key {
        c.profile.unwrap_or_else(|| self.me())
    }

    /// The incognito identity this contact talks to, if any.
    pub fn profile_of(&self, c: &Contact) -> Option<&data::Profile> {
        let key = c.profile?;
        self.data.profiles.iter().find(|p| p.key == key)
    }

    /// The slot of the identity that talks to `contact`. `None` if its
    /// incognito identity is gone: never fall back to the main one, which
    /// would tie the two together.
    fn slot_for(&self, contact: &Ed25519Key) -> Option<usize> {
        let i = self.index(contact)?;
        match self.data.contacts[i].profile {
            None => Some(0),
            Some(key) => self.slot_of(&key),
        }
    }

    /// The slot of our identity `key`.
    fn slot_of(&self, key: &Ed25519Key) -> Option<usize> {
        if *key == self.me() {
            return Some(0);
        }
        let n = self.data.profiles.iter().position(|p| p.key == *key)?;
        Some(n + 1)
    }

    pub fn is_request(&self, contact: &Ed25519Key) -> bool {
        self.slot_for(contact)
            .is_some_and(|s| self.core_at(s).is_request(contact) == Some(true))
    }

    fn nickname_at(&self, slot: usize) -> String {
        match slot {
            0 => self.data.nickname.clone(),
            n => self.data.profiles[n - 1].nickname.clone(),
        }
    }

    /// A one-time invite: a fresh code that works for one request (ADR-013).
    pub fn invite(&mut self) -> String {
        let code = new_code();
        let once = &mut self.data.invites.once;
        once.push(code);
        if once.len() > MAX_ONE_TIME_CODES {
            once.remove(0);
        }
        self.persist();
        self.invite_with(0, code)
    }

    /// The reusable invite; `renew` replaces it and stops the old one.
    pub fn public_invite(&mut self, renew: bool) -> String {
        let code = match self.data.invites.public {
            Some(code) if !renew => code,
            _ => {
                let code = new_code();
                self.data.invites.public = Some(code);
                self.persist();
                code
            }
        };
        self.invite_with(0, code)
    }

    /// An invite of a new identity of its own, for one contact (ADR-014).
    /// An empty `nickname` picks two random words.
    pub fn incognito_invite(&mut self, nickname: &str) -> Result<String, String> {
        let waiting = self.waiting();
        if waiting.len() >= MAX_WAITING_PROFILES {
            self.retire(&waiting[0]);
        }
        let code = new_code();
        let slot = self.new_profile(nickname, Some(code))?;
        self.persist();
        Ok(self.invite_with(slot, code))
    }

    /// Incognito identities whose invite nobody used yet, oldest first.
    fn waiting(&self) -> Vec<Ed25519Key> {
        let p = self.data.profiles.iter();
        p.filter(|p| p.code.is_some()).map(|p| p.key).collect()
    }

    /// Identity `slot`'s UnifiedPush endpoint (ADR-012, ADR-014).
    fn push_mut(&mut self, slot: usize) -> &mut Option<String> {
        match slot {
            0 => &mut self.data.push,
            n => &mut self.data.profiles[n - 1].push,
        }
    }

    /// Identity `slot`'s fallback key state (ADR-002).
    fn fallback_mut(&mut self, slot: usize) -> (&mut bool, &mut Option<u64>) {
        match slot {
            0 => (
                &mut self.data.fallback_published,
                &mut self.data.old_fallback_since,
            ),
            n => {
                let p = &mut self.data.profiles[n - 1];
                (&mut p.fallback_published, &mut p.old_fallback_since)
            }
        }
    }

    /// A new incognito identity, connected; its slot.
    fn new_profile(&mut self, nickname: &str, code: Option<InviteCode>) -> Result<usize, String> {
        let nickname = match nickname.trim() {
            "" => password::words(2).to_string(),
            n => n.to_string(),
        };
        check_nickname(&nickname).map_err(|_| "nickname: 1-64 bytes, no control characters")?;
        let core = Arc::new(Mutex::new(Client::default()));
        // At once: the user is about to hand out its invite, or to use it.
        let live = start(
            core,
            &self.data,
            &self.path,
            &self.events_tx,
            Duration::ZERO,
            true,
        )?;
        let key = live
            .core
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .identity()
            .ed25519;
        self.data.profiles.push(data::Profile {
            key,
            nickname,
            code,
            // Below 2^62: room to count up, and no clash with another identity's.
            next_client_id: getrandom::u64().unwrap_or(0) >> 2,
            ..Default::default()
        });
        self.lives.push(live);
        Ok(self.lives.len() - 1)
    }

    /// Asks the relay to forget incognito identity `key` and erases it here.
    fn retire(&mut self, key: &Ed25519Key) {
        let Some(slot) = self.slot_of(key).filter(|&s| s > 0) else {
            return;
        };
        let live = self.lives.remove(slot);
        self.data.profiles.remove(slot - 1);
        let _ = live.relay.send(ClientFrame::Unregister);
        self.retiring
            .push((*key, live, Instant::now() + RETIRE_TIMEOUT));
    }

    /// Every one-time invite handed out so far stops working, incognito ones too.
    pub fn revoke_invites(&mut self) -> usize {
        let mut n = std::mem::take(&mut self.data.invites.once).len();
        let waiting = self.waiting();
        for key in &waiting {
            self.retire(key);
        }
        n += waiting.len();
        self.persist();
        n
    }

    pub fn set_codes_only(&mut self, on: bool) {
        self.data.invites.codes_only = on;
        self.persist();
    }

    fn invite_with(&self, slot: usize, code: InviteCode) -> String {
        Invite {
            relay_url: self.data.relay_url.clone(),
            onion_url: self.data.onion_url.clone(),
            identity: self.core_at(slot).identity(),
            code: Some(code),
        }
        .encode()
    }

    pub fn index(&self, key: &Ed25519Key) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.key == *key)
    }

    fn note(&mut self, text: impl Into<String>) {
        self.happened.push(Happened::Note(text.into()));
    }

    /// The last note, for callers that report it as their own error.
    fn take_note(&mut self) -> String {
        match self.happened.pop() {
            Some(Happened::Note(n)) => n,
            _ => String::new(),
        }
    }

    /// Must run after every change to the ratchet or the data.
    pub fn persist(&mut self) -> bool {
        if self.gone {
            return false;
        }
        let saved = self
            .lives
            .iter()
            .skip(1)
            .map(|l| {
                l.core
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .export()
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
            .and_then(|states| self.data.to_json(&states).map_err(|e| e.to_string()))
            .and_then(|app| self.core().save(&self.key, &app).map_err(|e| e.to_string()))
            .and_then(|bytes| data::write_atomic(&self.path, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = &saved {
            self.note(format!("SAVE FAILED: {e}"));
        }
        saved.is_ok()
    }

    fn system(&mut self, i: usize, text: impl Into<String>) {
        self.data.contacts[i].messages.push(Msg {
            mine: false,
            text: text.into(),
            at: now(),
            expires_at: None,
            status: Status::System,
        });
    }

    /// Encrypts, saves the advanced ratchet, then hands the envelope to the transport.
    /// An unsaved ratchet must never go on the wire: a restart would replay it.
    fn transmit(&mut self, key: &Ed25519Key, body: Body) -> Result<u64, String> {
        let slot = self.slot_for(key).ok_or(MISSING_PROFILE)?;
        let envelope = self
            .core_at(slot)
            .encrypt(key, body)
            .map_err(|e| e.to_string())?;
        let next = match slot {
            0 => &mut self.data.next_client_id,
            n => &mut self.data.profiles[n - 1].next_client_id,
        };
        let client_id = *next;
        *next += 1;
        if !self.persist() {
            return Err(self.take_note());
        }
        self.lives[slot]
            .relay
            .send(ClientFrame::Send {
                client_id,
                envelope,
            })
            .map_err(|_| "relay link closed")?;
        Ok(client_id)
    }

    fn claim(&mut self, key: Ed25519Key) {
        if let Some(slot) = self.slot_for(&key) {
            let _ = self.lives[slot]
                .relay
                .send(ClientFrame::ClaimKey { target: key });
        }
    }

    fn has_session(&self, key: &Ed25519Key) -> bool {
        self.slot_for(key)
            .is_some_and(|s| self.core_at(s).has_session(key))
    }

    /// Sends our contact request (if still owed) and every queued message.
    fn flush(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        let Some(slot) = self.slot_for(&key) else {
            return self.note(MISSING_PROFILE);
        };
        if self.data.contacts[i].request_pending {
            let (identity, nickname) = (self.core_at(slot).identity(), self.nickname_at(slot));
            let note = String::new();
            let body = match self.data.contacts[i].invite_code {
                Some(code) => Body::ContactRequestV2 {
                    identity,
                    nickname,
                    note,
                    code,
                },
                None => Body::ContactRequest {
                    identity,
                    nickname,
                    note,
                },
            };
            match self.transmit(&key, body) {
                Ok(_) => self.data.contacts[i].request_pending = false,
                Err(e) => return self.note(e),
            }
        }
        let expires_in = self.data.contacts[i].expires_in;
        for m in 0..self.data.contacts[i].messages.len() {
            let msg = &self.data.contacts[i].messages[m];
            if msg.status != Status::Queued {
                continue;
            }
            let body = Body::Text {
                sent_at: msg.at,
                expires_in,
                text: msg.text.clone(),
            };
            self.data.contacts[i].messages[m].status = match self.transmit(&key, body) {
                Ok(id) => Status::Sending(id),
                Err(e) => Status::Failed(e),
            };
        }
        self.persist();
    }

    /// One relay event of our identity `who`.
    pub fn on_relay(&mut self, (who, ev): RelayEvent) {
        if let Some(r) = self.retiring.iter().position(|(k, ..)| *k == who) {
            // Gone from the relay, or at least from here.
            if matches!(
                ev,
                Event::Frame(ServerFrame::Unregistered) | Event::Disconnected
            ) {
                self.retiring.remove(r);
            }
            return;
        }
        let Some(slot) = self.slot_of(&who) else {
            return;
        };
        match ev {
            Event::RelayVersion(v) => {
                let ours = parse_version(env!("CARGO_PKG_VERSION"));
                self.push_ok = parse_version(&v) >= Some(PUSH_SINCE);
                self.update = (parse_version(&v) > ours).then_some(v);
            }
            Event::Connected => {
                self.lives[slot].connected = true;
                if slot == 0 {
                    self.connected = true;
                }
                // The relay may have lost it, or this is a restored backup.
                let has_push = self.push_mut(slot).is_some();
                if has_push {
                    self.tell_push(slot);
                }
                let waiting: Vec<_> = self
                    .data
                    .contacts
                    .iter()
                    .filter(|c| {
                        c.request_pending || c.messages.iter().any(|m| m.status == Status::Queued)
                    })
                    .map(|c| c.key)
                    .filter(|k| self.slot_for(k) == Some(slot))
                    .collect();
                for key in waiting {
                    match self.has_session(&key) {
                        true => {
                            if let Some(i) = self.index(&key) {
                                self.flush(i);
                            }
                        }
                        false => self.claim(key),
                    }
                }
            }
            Event::Disconnected => {
                self.lives[slot].connected = false;
                if slot == 0 {
                    self.connected = false;
                    // An older relay closes the connection on `Unregister`.
                    if self.leaving.as_ref().is_some_and(|l| l.unregistering) {
                        self.finish_wipe(NOT_REMOVED);
                    }
                }
            }
            Event::Frame(frame) => self.on_frame(slot, frame),
        }
    }

    fn on_frame(&mut self, slot: usize, frame: ServerFrame) {
        match frame {
            ServerFrame::KeyCount {
                one_time_left,
                fallback_used,
            } => {
                let (published, since) = {
                    let (p, s) = self.fallback_mut(slot);
                    (*p, *s)
                };
                // ADR-002: a used fallback key is replaced, and the one before
                // it kept until nothing sent to it can still be queued.
                // vodozemac keeps only one previous key, so no new rotation
                // while one is waiting: claims alone (the relay reports every
                // one) must not push out a key a new contact may have used.
                // An empty relay (first run, or it lost its database) has no
                // fallback key either.
                let used = fallback_used || one_time_left == 0;
                let rotate = !published || (used && since.is_none());
                let keys = self.core_at(slot).prekeys(one_time_left as usize, rotate);
                if !keys.one_time.is_empty() || keys.fallback.is_some() {
                    let (p, s) = self.fallback_mut(slot);
                    *p = true;
                    if rotate && published {
                        *s = Some(now());
                    }
                    self.persist();
                    let _ = self.lives[slot].relay.send(ClientFrame::PublishKeys {
                        one_time: keys.one_time,
                        fallback: keys.fallback,
                    });
                }
            }
            ServerFrame::ClaimedKey { target, key } => {
                let Some(i) = self.index(&target) else { return };
                if self.slot_for(&target) != Some(slot) {
                    return;
                }
                if !self.has_session(&target) {
                    let started = self.core_at(slot).start_session(&target, &key);
                    if let Err(e) = started {
                        return self.note(format!("cannot start session: {e}"));
                    }
                    self.persist();
                }
                self.flush(i);
            }
            ServerFrame::Deliver {
                relay_id,
                sender,
                envelope,
            } => {
                // Not saved means not acked: the relay keeps it for the next connection.
                if self.receive(slot, sender, &envelope) {
                    let _ = self.lives[slot].relay.send(ClientFrame::Ack { relay_id });
                }
            }
            ServerFrame::Accepted { client_id } => {
                self.set_status(client_id, Status::Sent);
                self.goodbye_done(Some(client_id));
            }
            ServerFrame::Error {
                client_id: Some(id),
                code,
            } => {
                self.set_status(id, Status::Failed(why(code)));
                self.goodbye_done(Some(id));
            }
            ServerFrame::Error {
                client_id: None,
                code,
            } => self.note(format!("relay: {}", why(code))),
            ServerFrame::Unregistered => {
                // The wipe ends once the relay forgot every identity.
                let me = self.core_at(slot).identity().ed25519;
                let Some(l) = &mut self.leaving else { return };
                l.registered.retain(|k| *k != me);
                if l.registered.is_empty() {
                    self.finish_wipe("");
                }
            }
            ServerFrame::Reported => self.note("reported to the relay operator, and blocked"),
            ServerFrame::Challenge { .. } | ServerFrame::AuthOk | ServerFrame::PushSet => {}
        }
    }

    /// ADR-009: goodbyes to every contact we talk to, then `Unregister` for
    /// every identity, then the wipe; `false` if the relay cannot be told (a
    /// `WipeAnyway` follows). `moved` (section 6) only erases this device, silently.
    pub fn leave(&mut self, moved: bool) -> bool {
        if moved {
            self.finish_wipe("");
            return true;
        }
        if !self.lives.iter().all(|l| l.connected) {
            self.happened.push(Happened::WipeAnyway(
                "Not every identity is connected to the relay yet; wait a minute, or wipe anyway."
                    .into(),
            ));
            return false;
        }
        let told: Vec<_> = self
            .data
            .contacts
            .iter()
            .filter(|c| !c.blocked && !c.left && !c.request_pending)
            .map(|c| c.key)
            .collect();
        let mut waiting = Vec::new();
        for key in told {
            let talking = self.has_session(&key) && !self.is_request(&key);
            if talking && let Ok(id) = self.transmit(&key, Body::Goodbye) {
                waiting.push(id);
            }
        }
        let registered = self
            .lives
            .iter()
            .map(|l| {
                l.core
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .identity()
                    .ed25519
            })
            .collect();
        self.leaving = Some(Leaving {
            waiting,
            until: Instant::now() + LEAVE_TIMEOUT,
            unregistering: false,
            registered,
        });
        self.goodbye_done(None);
        true
    }

    /// The relay answered `client_id`; once every goodbye is in, ask it to forget us.
    fn goodbye_done(&mut self, client_id: Option<u64>) {
        let Some(l) = &mut self.leaving else { return };
        l.waiting.retain(|w| Some(*w) != client_id);
        if l.waiting.is_empty() && !l.unregistering {
            l.unregistering = true;
            // The main one last: its answer finishes the wipe.
            for live in self.lives.iter().rev() {
                let _ = live.relay.send(ClientFrame::Unregister);
            }
        }
    }
    fn leave_timeout(&mut self) {
        let Some(l) = &self.leaving else { return };
        if Instant::now() < l.until {
            return;
        }
        match l.unregistering {
            true => self.finish_wipe(NOT_REMOVED),
            false => {
                self.leaving = None;
                self.happened.push(Happened::WipeAnyway(
                    "The relay did not answer in time; some contacts may already know.".into(),
                ));
            }
        }
    }

    /// Erases the device; `note` goes on the first-run screen.
    pub fn finish_wipe(&mut self, note: &str) {
        self.leaving = None;
        self.happened.push(match data::wipe(&self.path) {
            Ok(()) => {
                self.gone = true;
                Happened::Wiped(note.into())
            }
            Err(e) => Happened::Notice(format!("wipe failed: {e}")),
        });
    }

    fn set_status(&mut self, client_id: u64, status: Status) {
        for m in self
            .data
            .contacts
            .iter_mut()
            .flat_map(|c| c.messages.iter_mut())
        {
            if m.status == Status::Sending(client_id) {
                m.status = status;
                self.persist();
                return;
            }
        }
    }

    /// Whether a stranger's request to our identity `slot` may in; spends the
    /// code (ADR-013). An incognito identity takes only its own invite's code,
    /// once: then it belongs to that contact (ADR-014).
    fn admit(&mut self, slot: usize, body: &Body) -> bool {
        if slot == 0 {
            return self.data.invites.admit(body);
        }
        let p = &mut self.data.profiles[slot - 1];
        let ok = match (body, p.code) {
            (Body::ContactRequestV2 { code, .. }, Some(mine)) => bool::from(mine.ct_eq(code)),
            _ => false,
        };
        if ok {
            p.code = None;
        }
        ok
    }

    /// Decrypts and stores one delivery to our identity `slot`. `true` once it
    /// is safe to ack: the message is on disk, or there was nothing worth keeping.
    fn receive(&mut self, slot: usize, sender: Ed25519Key, envelope: &proto::Envelope) -> bool {
        // Blocked or rejected: not even decrypted. The caller still acks so the relay forgets it.
        if self.data.rejected.contains(&sender)
            || self
                .index(&sender)
                .is_some_and(|i| self.data.contacts[i].blocked)
        {
            return true;
        }
        // A contact of another of our identities writing here: they must stay apart.
        if self.index(&sender).is_some() && self.slot_for(&sender) != Some(slot) {
            return true;
        }
        let had_session = self.core_at(slot).has_session(&sender);
        let result = self.core_at(slot).decrypt(&sender, envelope);
        let got = match result {
            Ok(got) => got,
            Err(CoreError::Duplicate) => return true,
            // The status bar, not the chat: the sender is the relay's word,
            // and a relay must not be able to plant doubts in any chat.
            Err(e) => {
                if self.index(&sender).is_some() {
                    self.note(format!("a message could not be decrypted ({e})"));
                }
                return true;
            }
        };
        let i = match self.index(&sender) {
            Some(i) => i,
            // Silently: they cannot tell a refusal from a request not yet seen.
            None if !self.admit(slot, &got.body) => {
                self.core_at(slot).remove_contact(&sender);
                self.persist();
                return true;
            }
            None => {
                self.data.contacts.push(Contact {
                    key: sender,
                    added_at: now(),
                    profile: (slot > 0).then(|| self.data.profiles[slot - 1].key),
                    ..Default::default()
                });
                self.data.contacts.len() - 1
            }
        };
        if got.new_session && had_session {
            self.system(
                i,
                "⚠ they restarted the conversation (restored a backup?); messages may be lost",
            );
        }
        if got.missed > 0 {
            // The number comes from the peer's `seq`; do not let it look alarming on purpose.
            let missed = match got.missed {
                0..=9999 => got.missed.to_string(),
                _ => "over 9999".to_string(),
            };
            self.system(i, format!("⚠ {missed} message(s) missing so far"));
        }
        let alert = |request| Happened::Alert {
            key: sender,
            request,
        };
        match got.body {
            // A stranger's texts wait out of sight until the request is accepted.
            Body::Text { text, .. } if got.request => {
                let c = &mut self.data.contacts[i];
                match c.hidden.len() < MAX_HIDDEN {
                    true => c.hidden.push(Msg {
                        mine: false,
                        text,
                        at: now(),
                        expires_at: None,
                        status: Status::Received,
                    }),
                    false => c.held_back = c.held_back.saturating_add(1),
                }
            }
            Body::ContactAccept { .. } if got.request => {}
            Body::Text {
                expires_in, text, ..
            } => {
                let at = now();
                self.data.contacts[i].messages.push(Msg {
                    mine: false,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Received,
                });
                self.happened.push(alert(false));
            }
            Body::ContactRequest { note, .. } | Body::ContactRequestV2 { note, .. } => {
                let request = self.core_at(slot).is_request(&sender);
                if request == Some(true) {
                    self.happened.push(alert(true));
                }
                let text = match request {
                    Some(true) => format!(
                        "contact request{}: accept it, or delete to reject",
                        match note.is_empty() {
                            true => String::new(),
                            false => format!(": {note}"),
                        }
                    ),
                    _ => "connected".into(),
                };
                self.system(i, text);
            }
            Body::ContactAccept { .. } => self.system(i, "accepted your request"),
            Body::Goodbye if got.request => {}
            Body::Goodbye => {
                self.system(i, "deleted their account; this chat is read-only now");
                self.data.contacts[i].left = true;
                self.happened.push(alert(false));
            }
        }
        if self.open != Some(sender) {
            self.data.contacts[i].unread += 1;
        }
        self.persist()
    }

    /// Timers: disappearing messages, the rotated-out fallback key, a wipe
    /// waiting for the relay. Call about once a second.
    pub fn tick(&mut self) {
        self.expire();
        self.leave_timeout();
        let t = Instant::now();
        self.retiring.retain(|(_, _, until)| *until > t);
    }

    /// Drops messages whose disappearing timer ran out, and the rotated-out
    /// fallback keys once nothing sent to them can still be queued (ADR-002).
    fn expire(&mut self) {
        let t = now();
        let mut changed = false;
        let keep = (proto::QUEUE_TTL_DAYS + 1) * 86_400;
        let due = |since: Option<u64>| since.is_some_and(|since| t >= since + keep);
        if due(self.data.old_fallback_since) {
            self.core().forget_old_fallback();
            self.data.old_fallback_since = None;
            changed = true;
        }
        for n in 0..self.data.profiles.len() {
            if due(self.data.profiles[n].old_fallback_since) {
                self.core_at(n + 1).forget_old_fallback();
                self.data.profiles[n].old_fallback_since = None;
                changed = true;
            }
        }
        for c in &mut self.data.contacts {
            let before = c.messages.len();
            c.messages.retain(|m| m.expires_at.is_none_or(|e| e > t));
            changed |= c.messages.len() != before;
        }
        if changed {
            self.persist();
        }
    }

    /// Accepts contact `i`'s request: their held texts appear, they are told.
    pub fn accept(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        let Some(slot) = self.slot_for(&key) else {
            return;
        };
        if !self.core_at(slot).accept(&key) {
            return;
        }
        let body = Body::ContactAccept {
            nickname: self.nickname_at(slot),
        };
        let c = &mut self.data.contacts[i];
        let hidden = std::mem::take(&mut c.hidden);
        let held = std::mem::take(&mut c.held_back);
        c.messages.extend(hidden);
        if held > 0 {
            self.system(
                i,
                format!("{held} more message(s) from before you accepted were not kept"),
            );
        }
        match self.transmit(&key, body) {
            Ok(_) => self.system(i, "you accepted the request"),
            Err(e) => self.note(e),
        }
    }

    /// Changes one contact's settings and saves.
    pub fn edit(&mut self, i: usize, change: impl FnOnce(&mut Contact)) {
        if let Some(c) = self.data.contacts.get_mut(i) {
            change(c);
            self.persist();
        }
    }

    /// Their last few messages, joined, within the protocol limit (ADR-006).
    pub fn report_text(&self, i: usize) -> Option<String> {
        const LAST: usize = 5;
        let c = self.data.contacts.get(i)?;
        let mut theirs: Vec<&str> = c
            .messages
            .iter()
            .rev()
            .filter(|m| m.status == Status::Received)
            .take(LAST)
            .map(|m| m.text.as_str())
            .collect();
        theirs.reverse();
        let mut text = theirs.join("\n---\n");
        if text.len() > proto::MAX_TEXT_BYTES {
            let mut end = proto::MAX_TEXT_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        Some(text).filter(|t| !t.is_empty())
    }

    /// Sends `text` (shown to the user first) to the relay operator and blocks contact `i`.
    pub fn report(&mut self, i: usize, text: String) {
        let Some(sender) = self.data.contacts.get(i).map(|c| c.key) else {
            return;
        };
        // From the identity they wrote to: the relay knows them only there.
        if let Some(slot) = self.slot_for(&sender) {
            let _ = self.lives[slot]
                .relay
                .send(ClientFrame::Report { sender, text });
        }
        self.data.contacts[i].blocked = true;
        self.persist();
    }

    /// Removes contact `i`. Turning down a request is remembered, so it cannot
    /// simply ask again. An incognito contact takes its identity with it: a
    /// goodbye, then the relay forgets that key (ADR-014).
    pub fn delete(&mut self, i: usize) {
        let Some(c) = self.data.contacts.get(i) else {
            return;
        };
        let (key, profile, quiet) = (c.key, c.profile, c.blocked || c.left);
        let slot = self.slot_for(&key);
        if profile.is_some() && !quiet && self.has_session(&key) && !self.is_request(&key) {
            let _ = self.transmit(&key, Body::Goodbye);
        }
        let was_request = self.is_request(&key);
        self.data.contacts.remove(i);
        if let Some(slot) = slot {
            self.core_at(slot).remove_contact(&key);
        }
        match profile {
            Some(p) => self.retire(&p),
            None if was_request && !self.data.rejected.contains(&key) => {
                if self.data.rejected.len() >= MAX_REJECTED {
                    self.data.rejected.remove(0);
                }
                self.data.rejected.push(key);
            }
            None => {}
        }
        self.persist();
    }

    /// Queues `text` to contact `i` and sends it as soon as there is a session.
    pub fn send(&mut self, i: usize, text: &str) -> Result<(), String> {
        let Some(c) = self.data.contacts.get(i) else {
            return Err("add a contact first".into());
        };
        if text.len() > proto::MAX_TEXT_BYTES {
            return Err("message too long".into());
        }
        if c.blocked {
            return Err("contact is blocked; unblock them to write".into());
        }
        if c.left {
            return Err("this contact left".into());
        }
        let (key, expires_in) = (c.key, c.expires_in);
        let at = now();
        self.data.contacts[i].messages.push(Msg {
            mine: true,
            text: text.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
        });
        match self.has_session(&key) {
            true => self.flush(i),
            false => {
                self.persist();
                self.claim(key);
            }
        }
        Ok(())
    }

    /// Switches Tor on or off and reconnects every identity; returns what to tell the user.
    pub fn toggle_tor(&mut self) -> String {
        if !self.data.tor && self.data.onion_url.is_none() {
            return "This relay has no onion URL, so Tor is not available.".into();
        }
        self.data.tor = !self.data.tor;
        if let Err(e) = self.relink() {
            self.data.tor = !self.data.tor;
            return e;
        }
        self.persist();
        match self.data.tor {
            true => {
                "Tor on: connecting only through Tor. The first connection takes 10-30 s.".into()
            }
            false => "Tor off: the relay and your provider now see your IP address.".into(),
        }
    }

    /// Reconnects every identity with the current settings.
    fn relink(&mut self) -> Result<(), String> {
        let cores: Vec<_> = self.lives.iter().map(|l| Arc::clone(&l.core)).collect();
        self.lives = self.start_all(&cores)?;
        self.connected = false;
        Ok(())
    }

    /// A link per identity, in slot order.
    fn start_all(&self, cores: &[Arc<Mutex<Client>>]) -> Result<Vec<Live>, String> {
        let mut lives = Vec::new();
        for (slot, core) in cores.iter().enumerate() {
            let delay = if slot == 0 { Duration::ZERO } else { spread() };
            let (data, path, tx) = (&self.data, &self.path, &self.events_tx);
            lives.push(start(Arc::clone(core), data, path, tx, delay, slot > 0)?);
        }
        Ok(lives)
    }

    /// Replaces the bridge lines, one per line; empty turns bridges off
    /// (ADR-015). `Ok` says what happens now.
    pub fn set_bridges(&mut self, text: &str) -> Result<String, String> {
        let mut lines = Vec::new();
        let mut kinds: Vec<String> = Vec::new();
        let all = text.split(['\r', '\n']).map(str::trim);
        for (n, line) in all.filter(|l| !l.is_empty()).enumerate() {
            let line = line.trim_start_matches("Bridge ").trim();
            let Some(kind) = crate::bridge_transport(line) else {
                return Err(format!(
                    "line {}: not a bridge line termchat can use ({})",
                    n + 1,
                    crate::TRANSPORTS.join(", ")
                ));
            };
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
            // Pasting the same list twice must not double it.
            if !lines.iter().any(|l| l == line) {
                lines.push(line.to_string());
            }
        }
        if !lines.is_empty() && !cfg!(feature = "tor") {
            return Err("this build has no Tor support".into());
        }
        if lines != self.data.bridges {
            // Arti keeps the bridge lines in its folder in the clear. Stop
            // every link and move to a new folder; the old one is erased
            // below, once its client lets go.
            let cores: Vec<_> = self.lives.iter().map(|l| Arc::clone(&l.core)).collect();
            self.lives.clear();
            crate::forget_tor();
            self.data.tor_generation += 1;
            let old = std::mem::replace(&mut self.data.bridges, lines);
            match self.start_all(&cores) {
                Ok(lives) => self.lives = lives,
                // Back to the old bridges, which worked: never off Tor.
                Err(e) => {
                    self.data.bridges = old;
                    self.lives = self.start_all(&cores)?;
                    return Err(e);
                }
            }
            self.connected = false;
        }
        self.persist();
        data::prune_tor_dirs(&self.path, self.data.tor_generation);
        let n = self.data.bridges.len();
        Ok(match (n, pt_program(&self.data)) {
            (0, _) => "No bridges: Tor connects directly to the Tor network.".into(),
            (_, None) => format!(
                "{n} bridge(s) saved, but lyrebird was not found: install it, or set its path."
            ),
            _ if !self.data.tor => format!("{n} bridge(s) saved; they are used once Tor is on."),
            _ => format!(
                "{n} bridge(s) ({}): Tor now enters only through them. The first connection can take a few minutes.",
                kinds.join(", ")
            ),
        })
    }

    /// The bridges, without their secrets: transport and address.
    pub fn bridges(&self) -> Vec<String> {
        let shown = |l: &String| l.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        self.data.bridges.iter().map(shown).collect()
    }

    /// Writes a backup of the identities and contacts (no messages) to `to`.
    pub fn backup(&mut self, to: &Path, password: &str) -> Result<(), String> {
        if let Some(why) = password::weakness(password, &[&self.data.nickname]) {
            return Err(why);
        }
        let key =
            create_file_key(password.as_bytes(), KdfParams::DEFAULT).map_err(|e| e.to_string())?;
        let mut copy = self.data.clone();
        copy.backed_up = true;
        // No messages at all, a pending request's held texts included.
        copy.contacts.iter_mut().for_each(|c| {
            c.messages.clear();
            c.hidden.clear();
            c.held_back = 0;
        });
        let states = (1..self.lives.len())
            .map(|s| self.core_at(s).without_sessions().export())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let app = copy.to_json(&states).map_err(|e| e.to_string())?;
        let bytes = self
            .core()
            .without_sessions()
            .save(&key, &app)
            .map_err(|e| e.to_string())?;
        data::write_atomic(to, &bytes).map_err(|e| e.to_string())?;
        self.data.backed_up = true;
        self.persist();
        data::remember_backup(&self.path, to, &bytes)
            .map_err(|e| format!("backup written, but a wipe will not know it: {e}"))
    }

    fn check_current(&self, current: &str) -> Result<(), String> {
        let file = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        store::open(current.as_bytes(), &file)
            .map(|_| ())
            .map_err(|_| "current password is wrong".into())
    }

    /// Sets the duress password, or turns it off with `None`. `Ok` says what happened.
    pub fn set_duress(&mut self, current: &str, duress: Option<&str>) -> Result<String, String> {
        self.check_current(current)?;
        let Some(duress) = duress else {
            set_duress(&mut self.key, None).map_err(|e| e.to_string())?;
            self.persist();
            return Ok("Duress password turned off.".into());
        };
        if password::distance(current, duress) < DURESS_DISTANCE {
            return Err(format!(
                "must differ from your password by at least {DURESS_DISTANCE} characters"
            ));
        }
        if let Some(why) = password::weakness(duress, &[current, &self.data.nickname]) {
            return Err(why);
        }
        set_duress(&mut self.key, Some(duress.as_bytes())).map_err(|e| e.to_string())?;
        self.persist();
        Ok("Duress password set. Entering it at unlock erases everything.".into())
    }

    pub fn change_password(&mut self, current: &str, new: &str) -> Result<(), String> {
        self.check_current(current)?;
        if let Some(why) = password::weakness(new, &[current, &self.data.nickname]) {
            return Err(why);
        }
        if new == current {
            return Err("that is the current password".into());
        }
        // The duress password itself is unknown here, only its slot.
        if termchat_core::opens_duress(&self.key, new.as_bytes()).map_err(|e| e.to_string())? {
            return Err("that is your duress password".into());
        }
        termchat_core::rekey(&mut self.key, new.as_bytes()).map_err(|e| e.to_string())?;
        if !self.persist() {
            return Err(self.take_note());
        }
        Ok(())
    }

    /// Adds a contact from an invite string and starts the handshake; its
    /// index. `incognito` talks to them from a new identity of its own (ADR-014).
    pub fn add(&mut self, invite: &str, incognito: bool) -> Result<usize, String> {
        let invite = Invite::decode(invite).map_err(|e| format!("invalid invite: {e}"))?;
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
        let onion_matches =
            matches!((&invite.onion_url, &self.data.onion_url), (Some(a), Some(b)) if same(a, b));
        if !same(&invite.relay_url, &self.data.relay_url) && !onion_matches {
            return Err(format!(
                "they use another relay ({}); only one relay is supported for now",
                clean(&invite.relay_url)
            ));
        }
        let key = invite.identity.ed25519;
        if self.slot_of(&key).is_some() {
            return Err("that is your own invite".into());
        }
        let existing = self.index(&key);
        if incognito && existing.is_some() {
            return Err("already a contact: incognito works only for someone new".into());
        }
        let slot = match (incognito, existing) {
            (true, _) => self.new_profile("", None)?,
            (false, Some(i)) => self
                .slot_for(&self.data.contacts[i].key)
                .ok_or(MISSING_PROFILE)?,
            (false, None) => 0,
        };
        let added = self.core_at(slot).add_contact(&invite.identity);
        if let Err(e) = added {
            if incognito {
                let p = self.data.profiles[slot - 1].key;
                self.retire(&p);
            }
            return Err(format!("invalid invite: {e}"));
        }
        // Adding their invite is how a rejection is taken back.
        self.data.rejected.retain(|k| *k != key);
        let i = match existing {
            Some(i) => i,
            None => {
                self.data.contacts.push(Contact {
                    key,
                    request_pending: true,
                    added_at: now(),
                    profile: (slot > 0).then(|| self.data.profiles[slot - 1].key),
                    ..Default::default()
                });
                self.data.contacts.len() - 1
            }
        };
        // A new code from someone who never answered: our request may have
        // been dropped with its one-time key spent, so start over (ADR-013).
        let c = &self.data.contacts[i];
        if invite.code.is_some() && c.invite_code != invite.code && !c.request_pending {
            let mut core = self.core_at(slot);
            if core.nickname(&key).is_none() {
                core.remove_contact(&key);
                core.add_contact(&invite.identity)
                    .map_err(|e| format!("invalid invite: {e}"))?;
                drop(core);
                self.data.contacts[i].request_pending = true;
            }
        }
        self.data.contacts[i].invite_code = invite.code;
        self.persist();
        self.claim(key);
        Ok(i)
    }

    /// What the contact is called here. Names are chosen by the peer, so an
    /// unverified contact, or one whose name collides with another contact or with
    /// ours, also shows the start of its fingerprint: "Alice·48213" (ADR-007).
    pub fn name(&self, c: &Contact) -> String {
        let base = |c: &Contact| {
            let nick = self
                .slot_for(&c.key)
                .and_then(|s| self.core_at(s).nickname(&c.key).map(str::to_owned));
            clean(&c.petname.clone().or(nick).unwrap_or_default())
        };
        let fp = termchat_core::fingerprint(&c.key);
        let name = base(c);
        if name.is_empty() {
            return fp[..5].to_string();
        }
        // ponytail: O(contacts) per name, O(n²) per frame; cache if lists grow large.
        let collides = name == clean(&self.data.nickname)
            || self
                .data
                .contacts
                .iter()
                .any(|o| o.key != c.key && base(o) == name);
        match !c.verified || collides {
            true => format!("{name}·{}", &fp[..5]),
            false => name,
        }
    }

    /// Where the relay should wake identity `who` (ours); `None` stops it
    /// (ADR-012). Each identity has its own endpoint (ADR-014).
    pub fn set_push(&mut self, who: &Ed25519Key, endpoint: Option<String>) {
        let Some(slot) = self.slot_of(who) else {
            return;
        };
        *self.push_mut(slot) = endpoint;
        self.persist();
        self.tell_push(slot);
    }

    fn tell_push(&mut self, slot: usize) {
        let endpoint = self.push_mut(slot).clone();
        if self.lives[slot].connected && self.push_ok {
            let _ = self.lives[slot]
                .relay
                .send(ClientFrame::SetPush { endpoint });
        }
    }

    /// Marks this version's notes as read.
    pub fn news_seen(&mut self) {
        self.data.news_seen = env!("CARGO_PKG_VERSION").into();
        self.persist();
    }

    /// Most recent first.
    pub fn resort(&mut self) {
        self.data
            .contacts
            .sort_by_key(|c| std::cmp::Reverse(c.last_at()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_text_keeps_lines_but_not_bidi_overrides() {
        assert_eq!(clean_text("a\u{202E}b\nc\u{0007}d"), "ab\ncd");
    }

    #[test]
    fn plain_ws_only_for_local_or_onion() {
        for ok in [
            "wss://relay.example.org/",
            "ws://localhost:8080/",
            "ws://127.0.0.1:8080",
            "ws://[::1]:8080/",
            "ws://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/",
        ] {
            assert!(relay_url_ok(ok), "{ok}");
        }
        for bad in [
            "ws://relay.example.org/",
            "ws://10.0.0.5:8080/",
            "ws://localhost.evil.org/",
            "http://localhost/",
            "",
            // The host after `@`, `?` or `#` is what the transport connects to.
            "ws://localhost:8080@evil.org/",
            "ws://[::1]@evil.org/",
            "ws://evil.org#.onion",
            "ws://evil.org?x=.onion",
            "ws://evil.org/.onion",
        ] {
            assert!(!relay_url_ok(bad), "{bad}");
        }
        assert_eq!(ws_host("ws://x.onion/.onion/").as_deref(), Some("x.onion"));
        assert_eq!(
            ws_host("ws://evil.org/x.onion").as_deref(),
            Some("evil.org")
        );
    }
}
