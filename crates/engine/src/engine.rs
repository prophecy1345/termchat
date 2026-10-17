//! An unlocked account: the state, the relay link and everything that changes
//! them. The interface calls methods and drains `happened` afterwards.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use termchat_core::proto::{
    self, Body, ClientFrame, Ed25519Key, ErrorCode, Invite, ServerFrame, check_nickname,
};
use termchat_core::{
    Client, Error as CoreError, FileKey, KdfParams, create_file_key, set_duress, store,
};
use tokio::sync::mpsc::Receiver;
use tokio_tungstenite::tungstenite::http::Uri;

use crate::data::{self, Contact, Data, Msg, Status};
use crate::{Dial, Error, Event, Relay, parse_version, password};

/// How long a wipe waits for the relay to take the goodbyes (ADR-009).
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);
const NOT_REMOVED: &str =
    "The relay did not confirm removing your key; it forgets idle keys after 90 days.";
/// ADR-007: what a not-yet-accepted request may leave behind.
const MAX_HIDDEN: usize = 5;
const MAX_REJECTED: usize = 1000;
/// ADR-003: a typo in the real password must never hit the duress one.
pub const DURESS_DISTANCE: usize = 4;
/// Disappearing timer steps, in the order a button cycles them.
pub const TIMERS: [Option<u32>; 5] = [None, Some(300), Some(3600), Some(86_400), Some(604_800)];

pub fn next_timer(now: Option<u32>) -> Option<u32> {
    let at = TIMERS.iter().position(|t| *t == now).unwrap_or(0);
    TIMERS[(at + 1) % TIMERS.len()]
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
    pub core: Arc<Mutex<Client>>,
    key: FileKey,
    pub data: Data,
    relay: Relay,
    pub events: Receiver<Event>,
    pub connected: bool,
    /// A newer version the relay runs (ADR-010).
    pub update: Option<String>,
    /// The chat on screen: its messages do not count as unread.
    pub open: Option<Ed25519Key>,
    pub happened: Vec<Happened>,
    path: PathBuf,
    leaving: Option<Leaving>,
    /// Erased: nothing may write the state file again.
    gone: bool,
}

/// Goodbyes sent and not yet accepted, then `Unregister` (ADR-009).
struct Leaving {
    waiting: Vec<u64>,
    until: Instant,
    unregistering: bool,
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

/// Starts the relay link: through Tor only if Tor is on (ADR-004).
fn link(
    core: &Arc<Mutex<Client>>,
    data: &Data,
    path: &Path,
) -> Result<(Relay, Receiver<Event>), String> {
    let identity = core
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .identity();
    let signer = Arc::clone(core);
    let sign = Box::new(move |host: &str, nonce: &[u8; 32]| {
        signer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .auth_signature(host, nonce)
    });
    let (url, dial) = match (&data.onion_url, data.tor) {
        (Some(onion), true) => (onion.as_str(), Dial::Tor(data::tor_dir(path))),
        (None, true) => return Err("Tor is on but the relay has no onion URL".into()),
        (_, false) => (data.relay_url.as_str(), Dial::Direct),
    };
    Relay::connect(url, identity, sign, dial).map_err(|e| match e {
        Error::NoTor => "this build has no Tor support".into(),
        _ => format!("bad relay URL: {url}"),
    })
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
                let _ = data::wipe(path);
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

    fn open(core: Client, key: FileKey, data: Data, path: PathBuf) -> Result<Box<Self>, String> {
        let core = Arc::new(Mutex::new(core));
        let (relay, events) = link(&core, &data, &path)?;
        let happened = data::permission_warning(&path)
            .map(Happened::Notice)
            .into_iter()
            .collect();
        Ok(Box::new(Self {
            core,
            key,
            data,
            relay,
            events,
            connected: false,
            update: None,
            open: None,
            happened,
            path,
            leaving: None,
            gone: false,
        }))
    }

    pub fn core(&self) -> MutexGuard<'_, Client> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn me(&self) -> Ed25519Key {
        self.core().identity().ed25519
    }

    pub fn invite(&self) -> String {
        Invite {
            version: proto::VERSION,
            relay_url: self.data.relay_url.clone(),
            onion_url: self.data.onion_url.clone(),
            identity: self.core().identity(),
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
            .data
            .to_json()
            .map_err(|e| e.to_string())
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
        let envelope = self.core().encrypt(key, body).map_err(|e| e.to_string())?;
        let client_id = self.data.next_client_id;
        self.data.next_client_id += 1;
        if !self.persist() {
            return Err(self.take_note());
        }
        self.relay
            .send(ClientFrame::Send {
                client_id,
                envelope,
            })
            .map_err(|_| "relay link closed")?;
        Ok(client_id)
    }

    fn claim(&mut self, key: Ed25519Key) {
        let _ = self.relay.send(ClientFrame::ClaimKey { target: key });
    }

    /// Sends our contact request (if still owed) and every queued message.
    fn flush(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        if self.data.contacts[i].request_pending {
            let body = Body::ContactRequest {
                identity: self.core().identity(),
                nickname: self.data.nickname.clone(),
                note: String::new(),
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

    pub fn on_relay(&mut self, ev: Event) {
        match ev {
            Event::RelayVersion(v) => {
                let ours = parse_version(env!("CARGO_PKG_VERSION"));
                self.update = (parse_version(&v) > ours).then_some(v);
            }
            Event::Connected => {
                self.connected = true;
                let waiting: Vec<_> = self
                    .data
                    .contacts
                    .iter()
                    .filter(|c| {
                        c.request_pending || c.messages.iter().any(|m| m.status == Status::Queued)
                    })
                    .map(|c| c.key)
                    .collect();
                for key in waiting {
                    let ready = self.core().has_session(&key);
                    match ready {
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
                self.connected = false;
                // An older relay closes the connection on `Unregister`.
                if self.leaving.as_ref().is_some_and(|l| l.unregistering) {
                    self.finish_wipe(NOT_REMOVED);
                }
            }
            Event::Frame(frame) => self.on_frame(frame),
        }
    }

    fn on_frame(&mut self, frame: ServerFrame) {
        match frame {
            ServerFrame::KeyCount {
                one_time_left,
                fallback_used,
            } => {
                // An empty relay (first run, or it lost its database) has no fallback key either.
                let rotate = fallback_used || !self.data.fallback_published || one_time_left == 0;
                let keys = self.core().prekeys(one_time_left as usize, rotate);
                if !keys.one_time.is_empty() || keys.fallback.is_some() {
                    if rotate && self.data.fallback_published {
                        self.data.old_fallback_since = Some(now());
                    }
                    self.data.fallback_published = true;
                    self.persist();
                    let _ = self.relay.send(ClientFrame::PublishKeys {
                        one_time: keys.one_time,
                        fallback: keys.fallback,
                    });
                }
            }
            ServerFrame::ClaimedKey { target, key } => {
                let Some(i) = self.index(&target) else { return };
                if !self.core().has_session(&target) {
                    let started = self.core().start_session(&target, &key);
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
                if self.receive(sender, &envelope) {
                    let _ = self.relay.send(ClientFrame::Ack { relay_id });
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
            ServerFrame::Unregistered => self.finish_wipe(""),
            ServerFrame::Reported => self.note("reported to the relay operator, and blocked"),
            ServerFrame::Challenge { .. } | ServerFrame::AuthOk | ServerFrame::PushSet => {}
        }
    }

    /// ADR-009: goodbyes to every contact we talk to, then `Unregister`, then
    /// the wipe; `false` if the relay cannot be told (a `WipeAnyway` follows).
    /// `moved` (section 6) only erases this device, silently.
    pub fn leave(&mut self, moved: bool) -> bool {
        if moved {
            self.finish_wipe("");
            return true;
        }
        if !self.connected {
            self.happened
                .push(Happened::WipeAnyway("No connection to the relay.".into()));
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
            let talking =
                self.core().has_session(&key) && self.core().is_request(&key) != Some(true);
            if talking && let Ok(id) = self.transmit(&key, Body::Goodbye) {
                waiting.push(id);
            }
        }
        self.leaving = Some(Leaving {
            waiting,
            until: Instant::now() + LEAVE_TIMEOUT,
            unregistering: false,
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
            let _ = self.relay.send(ClientFrame::Unregister);
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

    /// Decrypts and stores one delivery. `true` once it is safe to ack: the
    /// message is on disk, or there was nothing worth keeping.
    fn receive(&mut self, sender: Ed25519Key, envelope: &proto::Envelope) -> bool {
        // Blocked or rejected: not even decrypted. The caller still acks so the relay forgets it.
        if self.data.rejected.contains(&sender)
            || self
                .index(&sender)
                .is_some_and(|i| self.data.contacts[i].blocked)
        {
            return true;
        }
        let had_session = self.core().has_session(&sender);
        let result = self.core().decrypt(&sender, envelope);
        let got = match result {
            Ok(got) => got,
            Err(CoreError::Duplicate) => return true,
            Err(e) => {
                if let Some(i) = self.index(&sender) {
                    self.system(i, format!("a message could not be decrypted ({e})"));
                    self.persist();
                }
                return true;
            }
        };
        let i = match self.index(&sender) {
            Some(i) => i,
            None => {
                self.data.contacts.push(Contact {
                    key: sender,
                    added_at: now(),
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
            Body::ContactRequest { note, .. } => {
                let request = self.core().is_request(&sender);
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
    }

    /// Drops messages whose disappearing timer ran out, and the rotated-out
    /// fallback key once nothing sent to it can still be queued (ADR-002).
    fn expire(&mut self) {
        let t = now();
        let mut changed = false;
        let keep = (proto::QUEUE_TTL_DAYS + 1) * 86_400;
        if self
            .data
            .old_fallback_since
            .is_some_and(|since| t >= since + keep)
        {
            self.core().forget_old_fallback();
            self.data.old_fallback_since = None;
            changed = true;
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
        if !self.core().accept(&key) {
            return;
        }
        let body = Body::ContactAccept {
            nickname: self.data.nickname.clone(),
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
        let _ = self.relay.send(ClientFrame::Report { sender, text });
        self.data.contacts[i].blocked = true;
        self.persist();
    }

    /// Removes contact `i`. Turning down a request is remembered, so it cannot simply ask again.
    pub fn delete(&mut self, i: usize) {
        if i >= self.data.contacts.len() {
            return;
        }
        let c = self.data.contacts.remove(i);
        let was_request = self.core().is_request(&c.key) == Some(true);
        if was_request && !self.data.rejected.contains(&c.key) {
            if self.data.rejected.len() >= MAX_REJECTED {
                self.data.rejected.remove(0);
            }
            self.data.rejected.push(c.key);
        }
        self.core().remove_contact(&c.key);
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
        let ready = self.core().has_session(&key);
        match ready {
            true => self.flush(i),
            false => {
                self.persist();
                self.claim(key);
            }
        }
        Ok(())
    }

    /// Switches Tor on or off and reconnects; returns what to tell the user.
    pub fn toggle_tor(&mut self) -> String {
        if !self.data.tor && self.data.onion_url.is_none() {
            return "This relay has no onion URL, so Tor is not available.".into();
        }
        self.data.tor = !self.data.tor;
        match link(&self.core, &self.data, &self.path) {
            Ok((relay, events)) => {
                (self.relay, self.events) = (relay, events);
                self.connected = false;
                self.persist();
                match self.data.tor {
                    true => {
                        "Tor on: connecting only through Tor. The first connection takes 10-30 s."
                            .into()
                    }
                    false => "Tor off: the relay and your provider now see your IP address.".into(),
                }
            }
            Err(e) => {
                self.data.tor = !self.data.tor;
                e
            }
        }
    }

    /// Writes a backup of the identity and contacts (no messages) to `to`.
    pub fn backup(&mut self, to: &Path, password: &str) -> Result<(), String> {
        if let Some(why) = password::weakness(password, &[&self.data.nickname]) {
            return Err(why);
        }
        let key =
            create_file_key(password.as_bytes(), KdfParams::DEFAULT).map_err(|e| e.to_string())?;
        let mut copy = self.data.clone();
        copy.backed_up = true;
        copy.contacts.iter_mut().for_each(|c| c.messages.clear());
        let app = copy.to_json().map_err(|e| e.to_string())?;
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

    /// Adds a contact from an invite string and starts the handshake; its index.
    pub fn add(&mut self, invite: &str) -> Result<usize, String> {
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
        // Adding their invite is how a rejection is taken back.
        self.data.rejected.retain(|k| *k != key);
        if key == self.me() {
            return Err("that is your own invite".into());
        }
        self.core()
            .add_contact(&invite.identity)
            .map_err(|e| format!("invalid invite: {e}"))?;
        let i = match self.index(&key) {
            Some(i) => i,
            None => {
                self.data.contacts.push(Contact {
                    key,
                    request_pending: true,
                    added_at: now(),
                    ..Default::default()
                });
                self.data.contacts.len() - 1
            }
        };
        self.persist();
        self.claim(key);
        Ok(i)
    }

    /// What the contact is called here. Names are chosen by the peer, so an
    /// unverified contact, or one whose name collides with another contact or with
    /// ours, also shows the start of its fingerprint: "Alice·48213" (ADR-007).
    pub fn name(&self, c: &Contact) -> String {
        let base = |c: &Contact| {
            let nick = self.core().nickname(&c.key).map(str::to_owned);
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
