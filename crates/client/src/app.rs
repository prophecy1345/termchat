//! TUI state and event handling, without any drawing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termchat_core::proto::{
    self, Body, ClientFrame, Ed25519Key, Invite, ServerFrame, check_nickname,
};
use termchat_core::{
    Client, Error as CoreError, FileKey, KdfParams, create_file_key, set_duress, store,
};
use tokio::sync::mpsc::Receiver;
use zeroize::Zeroizing;

use crate::data::{self, Contact, Data, Msg, Status};
use crate::form::{self, Action, Form, Kind};
use crate::password;
use crate::{Dial, Event, Relay};

/// PLAN.md, M3.
pub const AUTOLOCK: Duration = Duration::from_secs(5 * 60);
/// ADR-003: a typo in the real password must never hit the duress one.
const DURESS_DISTANCE: usize = 4;
/// Disappearing timer steps for Ctrl+T.
const TIMERS: [Option<u32>; 5] = [None, Some(300), Some(3600), Some(86_400), Some(604_800)];

pub struct App {
    pub screen: Screen,
    pub path: PathBuf,
    pub quit: bool,
}

pub enum Screen {
    /// First run or restore from backup.
    Setup(Form),
    Unlock(Unlock),
    Main(Box<Main>),
}

#[derive(Default)]
pub struct Unlock {
    pub input: Zeroizing<String>,
    pub error: String,
}

pub enum Popup {
    Help,
    Notice(String),
    /// Ctrl+P: backup, duress password, wipe.
    Menu,
    Form(Form),
    Add(String),
    Invite(String),
    Verify,
    Rename(String),
    Delete,
    /// The exact text that would be disclosed to the relay operator.
    Report(String),
}

pub struct Main {
    pub core: Arc<Mutex<Client>>,
    key: FileKey,
    pub data: Data,
    relay: Relay,
    pub events: Receiver<Event>,
    pub connected: bool,
    pub selected: usize,
    pub input: String,
    pub popup: Option<Popup>,
    pub status: String,
    pub last_input: Instant,
    path: PathBuf,
    /// Everything was erased; the app returns to the first-run screen.
    wiped: bool,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let screen = match path.exists() {
            true => Screen::Unlock(Unlock::default()),
            false => Screen::Setup(Form::new(Kind::Setup)),
        };
        Self {
            screen,
            path,
            quit: false,
        }
    }

    /// Next relay event while unlocked; never resolves otherwise.
    pub async fn relay_event(&mut self) -> Event {
        if let Screen::Main(m) = &mut self.screen
            && let Some(ev) = m.events.recv().await
        {
            return ev;
        }
        std::future::pending().await
    }

    pub fn on_relay(&mut self, ev: Event) {
        if let Screen::Main(m) = &mut self.screen {
            m.on_relay(ev);
        }
    }

    pub fn on_tick(&mut self) {
        if let Screen::Main(m) = &mut self.screen {
            if m.last_input.elapsed() >= AUTOLOCK {
                self.lock();
            } else {
                m.expire();
            }
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c' | 'q')) {
            if let Screen::Main(m) = &mut self.screen {
                m.persist();
            }
            self.quit = true;
            return;
        }
        match &mut self.screen {
            Screen::Setup(f) if ctrl && k.code == KeyCode::Char('b') && f.kind == Kind::Setup => {
                self.screen = Screen::Setup(Form::new(Kind::Restore));
            }
            Screen::Setup(f) => match f.on_key(k) {
                Action::Cancel if f.kind == Kind::Restore => {
                    self.screen = Screen::Setup(Form::new(Kind::Setup))
                }
                Action::Submit if f.kind == Kind::Restore => match restore(f, &self.path) {
                    Ok(()) => {
                        let hint = "restored: enter the backup's password".to_string();
                        self.screen = Screen::Unlock(Unlock {
                            error: hint,
                            ..Unlock::default()
                        });
                    }
                    Err(e) => f.error = e,
                },
                Action::Submit => match setup(f, &self.path) {
                    Ok(main) => self.screen = Screen::Main(main),
                    Err(e) => f.error = e,
                },
                _ => {}
            },
            Screen::Unlock(u) => match u.on_key(k, &self.path) {
                Unlocked::Stay => {}
                Unlocked::Open(main) => self.screen = Screen::Main(main),
                Unlocked::Wiped => self.screen = Screen::Setup(Form::new(Kind::Setup)),
            },
            Screen::Main(m) => {
                m.last_input = Instant::now();
                if ctrl && k.code == KeyCode::Char('k') {
                    self.lock();
                } else {
                    m.on_key(k, ctrl);
                    if m.wiped {
                        self.screen = Screen::Setup(Form::new(Kind::Setup));
                    }
                }
            }
        }
    }

    /// Bracketed paste: the text goes into the active field as text only.
    /// Control characters are dropped so a pasted string cannot act as keys.
    pub fn on_paste(&mut self, text: &str) {
        let plain: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.screen {
            Screen::Setup(f) => f.paste(&plain),
            Screen::Unlock(u) => u.input.push_str(&plain),
            Screen::Main(m) => {
                m.last_input = Instant::now();
                match &mut m.popup {
                    Some(Popup::Add(s) | Popup::Rename(s)) => s.push_str(&plain),
                    Some(Popup::Form(f)) => f.paste(&plain),
                    Some(_) => {}
                    None => m.input.push_str(&crate::ui::clean(text)),
                }
            }
        }
    }

    /// Saves and drops every key and message from memory (PLAN.md, M3).
    pub fn lock(&mut self) {
        if let Screen::Main(m) = &mut self.screen {
            m.persist();
        }
        self.screen = Screen::Unlock(Unlock::default());
    }
}

/// Text input shared by forms and popups. `true` on Enter.
fn edit(s: &mut String, k: KeyEvent) -> bool {
    match k.code {
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => s.push(c),
        KeyCode::Backspace => {
            s.pop();
        }
        KeyCode::Enter => return true,
        _ => {}
    }
    false
}

fn setup(f: &Form, path: &Path) -> Result<Box<Main>, String> {
    let url = f.fields[0].trim();
    if !relay_url_ok(url) {
        return Err("relay URL must start with wss:// (ws:// only for localhost or .onion)".into());
    }
    let onion = f.fields[1].trim();
    let onion = (!onion.is_empty()).then(|| onion.to_string());
    if onion
        .as_ref()
        .is_some_and(|o| !(o.starts_with("ws://") && o.trim_end_matches('/').ends_with(".onion")))
    {
        return Err("onion URL looks like ws://xxxx.onion/".into());
    }
    let tor = match f.fields[2].trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => return Err("Use Tor: answer y or n".into()),
    };
    if tor && onion.is_none() {
        return Err("Tor needs the relay's onion URL (or answer n)".into());
    }
    if tor && !cfg!(feature = "tor") {
        return Err("this build has no Tor support; answer n".into());
    }
    let nickname = f.fields[3].trim();
    check_nickname(nickname).map_err(|_| "nickname: 1-64 bytes, no control characters")?;
    let password = f.new_password(4, &[nickname])?;
    let key =
        create_file_key(password.as_bytes(), KdfParams::DEFAULT).map_err(|e| e.to_string())?;
    let data = Data {
        relay_url: url.into(),
        onion_url: onion,
        tor,
        nickname: nickname.into(),
        ..Data::default()
    };
    let mut main = Main::open(Client::default(), key, data, path.to_owned())?;
    if !main.persist() {
        return Err(main.status);
    }
    main.popup = Some(Popup::Notice(
        "Back up your identity now: Ctrl+P, then b.\nWithout a backup, a lost device means a lost identity.".into(),
    ));
    Ok(main)
}

/// Plain `ws://` would show the relay traffic (and so who we talk to) to the
/// network; it is only fine where TLS adds nothing: the same machine or an
/// onion service, which authenticates and encrypts on its own.
fn relay_url_ok(url: &str) -> bool {
    if url.starts_with("wss://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("ws://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = authority
        .strip_prefix('[')
        .and_then(|h| h.split(']').next())
        .unwrap_or_else(|| authority.rsplit_once(':').map_or(authority, |(h, _)| h));
    matches!(host, "localhost" | "127.0.0.1" | "::1") || host.ends_with(".onion")
}

/// A backup is a state file without sessions or messages: restoring is a copy.
fn restore(f: &Form, path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(form::expand(&f.fields[0]))
        .map_err(|e| format!("cannot read backup: {e}"))?;
    data::write_atomic(path, &bytes).map_err(|e| e.to_string())
}

pub enum Unlocked {
    Stay,
    Open(Box<Main>),
    /// The duress password was entered and the data is gone.
    Wiped,
}

impl Unlock {
    fn on_key(&mut self, k: KeyEvent, path: &Path) -> Unlocked {
        if !edit(&mut self.input, k) {
            return Unlocked::Stay;
        }
        // Argon2 blocks the UI thread for ~1 s; move to spawn_blocking if it bothers.
        let loaded = std::fs::read(path)
            .map_err(|e| format!("cannot read state: {e}"))
            .map(|bytes| Client::load(self.input.as_bytes(), &bytes));
        self.input = Zeroizing::default();
        let result = match loaded {
            // ADR-003: erase first, say nothing.
            Ok(Err(CoreError::Duress)) => {
                let _ = data::wipe(path);
                return Unlocked::Wiped;
            }
            Ok(Err(CoreError::WrongPassword)) => Err("wrong password".to_string()),
            Ok(Err(e)) => Err(e.to_string()),
            Ok(Ok((core, key, app))) => Data::from_json(&app)
                .map_err(|e| e.to_string())
                .and_then(|data| Main::open(core, key, data, path.to_owned())),
            Err(e) => Err(e),
        };
        match result {
            Ok(main) => Unlocked::Open(main),
            Err(e) => {
                self.error = e;
                Unlocked::Stay
            }
        }
    }
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
        (Some(onion), true) => (onion.as_str(), Dial::Tor(path.with_file_name("arti"))),
        (None, true) => return Err("Tor is on but the relay has no onion URL".into()),
        (_, false) => (data.relay_url.as_str(), Dial::Direct),
    };
    Relay::connect(url, identity, sign, dial).map_err(|e| match e {
        crate::Error::NoTor => "this build has no Tor support".into(),
        _ => format!("bad relay URL: {url}"),
    })
}

impl Main {
    fn open(core: Client, key: FileKey, data: Data, path: PathBuf) -> Result<Box<Self>, String> {
        let core = Arc::new(Mutex::new(core));
        let (relay, events) = link(&core, &data, &path)?;
        Ok(Box::new(Self {
            core,
            key,
            data,
            relay,
            events,
            connected: false,
            selected: 0,
            input: String::new(),
            popup: None,
            status: data::permission_warning(&path).unwrap_or_default(),
            last_input: Instant::now(),
            path,
            wiped: false,
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

    /// Must run after every change to the ratchet or the data.
    fn persist(&mut self) -> bool {
        let saved = self
            .data
            .to_json()
            .map_err(|e| e.to_string())
            .and_then(|app| self.core().save(&self.key, &app).map_err(|e| e.to_string()))
            .and_then(|bytes| data::write_atomic(&self.path, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = &saved {
            self.status = format!("SAVE FAILED: {e}");
        }
        saved.is_ok()
    }

    fn index(&self, key: &Ed25519Key) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.key == *key)
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
            return Err(self.status.clone());
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
                Err(e) => return self.status = e,
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

    fn on_relay(&mut self, ev: Event) {
        match ev {
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
            Event::Disconnected => self.connected = false,
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
                        return self.status = format!("cannot start session: {e}");
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
            ServerFrame::Accepted { client_id } => self.set_status(client_id, Status::Sent),
            ServerFrame::Error {
                client_id: Some(id),
                code,
            } => self.set_status(id, Status::Failed(format!("{code:?}"))),
            ServerFrame::Error {
                client_id: None,
                code,
            } => self.status = format!("relay: {code:?}"),
            ServerFrame::Reported => {
                self.status = "reported to the relay operator, and blocked".into()
            }
            ServerFrame::Challenge { .. } | ServerFrame::AuthOk => {}
        }
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
        // Blocked: not even decrypted. The caller still acks so the relay forgets it.
        if self
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
        match got.body {
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
            }
            Body::ContactRequest { note, .. } => {
                let text = match self.core().is_request(&sender) {
                    Some(true) => format!(
                        "contact request{}  —  Ctrl+Y accept, Ctrl+D reject",
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
        }
        if i != self.selected {
            self.data.contacts[i].unread += 1;
        }
        self.persist()
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

    pub fn current(&self) -> Option<&Contact> {
        self.data.contacts.get(self.selected)
    }

    fn on_key(&mut self, k: KeyEvent, ctrl: bool) {
        if let Some(popup) = self.popup.take() {
            return self.on_popup_key(popup, k);
        }
        let key = self.current().map(|c| c.key);
        match (ctrl, k.code) {
            (false, KeyCode::F(1)) => self.popup = Some(Popup::Help),
            (false, KeyCode::Char('?')) if self.input.is_empty() => self.popup = Some(Popup::Help),
            (false, KeyCode::Up) => self.select(self.selected.saturating_sub(1)),
            (false, KeyCode::Down) => self.select(self.selected + 1),
            (true, KeyCode::Char('a')) => self.popup = Some(Popup::Add(String::new())),
            (true, KeyCode::Char('p')) => self.popup = Some(Popup::Menu),
            (true, KeyCode::Char('o')) => self.popup = Some(Popup::Invite(self.invite())),
            (true, KeyCode::Char('f')) if key.is_some() => self.popup = Some(Popup::Verify),
            (true, KeyCode::Char('r')) if key.is_some() => {
                self.popup = Some(Popup::Rename(String::new()))
            }
            (true, KeyCode::Char('d')) if key.is_some() => self.popup = Some(Popup::Delete),
            (true, KeyCode::Char('y')) => {
                if let Some(key) = key
                    && self.core().accept(&key)
                {
                    let body = Body::ContactAccept {
                        nickname: self.data.nickname.clone(),
                    };
                    match self.transmit(&key, body) {
                        Ok(_) => self.system(self.selected, "you accepted the request"),
                        Err(e) => self.status = e,
                    }
                }
            }
            (true, KeyCode::Char('t')) => {
                if let Some(c) = self.data.contacts.get_mut(self.selected) {
                    let at = TIMERS.iter().position(|t| *t == c.expires_in).unwrap_or(0);
                    c.expires_in = TIMERS[(at + 1) % TIMERS.len()];
                    self.persist();
                }
            }
            (true, KeyCode::Char('x')) => {
                if let Some(text) = self.report_text() {
                    self.popup = Some(Popup::Report(text));
                }
            }
            (true, KeyCode::Char('b')) => {
                if let Some(c) = self.data.contacts.get_mut(self.selected) {
                    c.blocked = !c.blocked;
                    let what = if c.blocked {
                        "blocked: their messages are dropped unread"
                    } else {
                        "unblocked"
                    };
                    self.status = what.into();
                    self.persist();
                }
            }
            (true, KeyCode::Char('e')) => {
                if let Some(c) = self.data.contacts.get_mut(self.selected) {
                    c.no_history = !c.no_history;
                    self.persist();
                }
            }
            (false, _) if edit(&mut self.input, k) => self.send(),
            _ => {}
        }
    }

    fn select(&mut self, i: usize) {
        if i < self.data.contacts.len() {
            self.selected = i;
            self.data.contacts[i].unread = 0;
        }
    }

    fn send(&mut self) {
        let text = std::mem::take(&mut self.input);
        let Some(c) = self.current() else {
            return self.status = "add a contact first (Ctrl+A)".into();
        };
        if text.trim().is_empty() {
            return;
        }
        if text.len() > proto::MAX_TEXT_BYTES {
            self.input = text;
            return self.status = "message too long".into();
        }
        if c.blocked {
            self.input = text;
            return self.status = "contact is blocked (Ctrl+B to unblock)".into();
        }
        let (key, expires_in, i) = (c.key, c.expires_in, self.selected);
        let at = now();
        self.data.contacts[i].messages.push(Msg {
            mine: true,
            text,
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
    }

    fn on_popup_key(&mut self, popup: Popup, k: KeyEvent) {
        let keep = match popup {
            Popup::Help | Popup::Notice(_) => None,
            Popup::Menu => match k.code {
                KeyCode::Char('b') => Some(Popup::Form(Form::new(Kind::Backup))),
                KeyCode::Char('d') => Some(Popup::Form(Form::new(Kind::Duress))),
                KeyCode::Char('w') => Some(Popup::Form(Form::new(Kind::Wipe))),
                KeyCode::Char('t') => Some(Popup::Notice(self.toggle_tor())),
                _ => None,
            },
            Popup::Form(mut f) => match f.on_key(k) {
                Action::Stay => Some(Popup::Form(f)),
                Action::Cancel => None,
                Action::Submit => match self.submit(&f) {
                    Ok(done) => Some(Popup::Notice(done)),
                    Err(e) => {
                        f.error = e;
                        Some(Popup::Form(f))
                    }
                },
            },
            Popup::Invite(invite) => {
                if k.code == KeyCode::Char('c') {
                    copy_to_clipboard(&invite);
                    self.status = "invite copied (if your terminal supports OSC 52)".into();
                }
                None
            }
            Popup::Verify => {
                if k.code == KeyCode::Char('v')
                    && let Some(c) = self.data.contacts.get_mut(self.selected)
                {
                    c.verified = !c.verified;
                    self.persist();
                }
                None
            }
            Popup::Report(text) => {
                if k.code == KeyCode::Char('y') && self.selected < self.data.contacts.len() {
                    let i = self.selected;
                    let sender = self.data.contacts[i].key;
                    let _ = self.relay.send(ClientFrame::Report { sender, text });
                    self.data.contacts[i].blocked = true;
                    self.persist();
                }
                None
            }
            Popup::Delete => {
                if k.code == KeyCode::Char('y') && self.selected < self.data.contacts.len() {
                    let c = self.data.contacts.remove(self.selected);
                    self.core().remove_contact(&c.key);
                    self.selected = self.selected.saturating_sub(1);
                    self.persist();
                }
                None
            }
            Popup::Add(mut s) if k.code != KeyCode::Esc => match edit(&mut s, k) {
                true => {
                    if let Err(e) = self.add(&s) {
                        self.status = e;
                    }
                    None
                }
                false => Some(Popup::Add(s)),
            },
            Popup::Rename(mut s) if k.code != KeyCode::Esc => match edit(&mut s, k) {
                true => {
                    let name = s.trim();
                    if let Some(c) = self.data.contacts.get_mut(self.selected) {
                        c.petname = (!name.is_empty()).then(|| name.chars().take(64).collect());
                    }
                    self.persist();
                    None
                }
                false => Some(Popup::Rename(s)),
            },
            Popup::Add(_) | Popup::Rename(_) => None,
        };
        self.popup = keep;
    }

    /// Their last few messages, joined, within the protocol limit (ADR-006).
    fn report_text(&self) -> Option<String> {
        const LAST: usize = 5;
        let c = self.current()?;
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

    /// Switches Tor on or off and reconnects; returns what to tell the user.
    fn toggle_tor(&mut self) -> String {
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

    /// Settings forms. `Ok` carries what to tell the user.
    fn submit(&mut self, f: &Form) -> Result<String, String> {
        match f.kind {
            Kind::Backup => {
                let path = form::expand(&f.fields[0]);
                let pw = f.new_password(1, &[&self.data.nickname])?;
                let key = create_file_key(pw.as_bytes(), KdfParams::DEFAULT)
                    .map_err(|e| e.to_string())?;
                let mut copy = self.data.clone();
                copy.backed_up = true;
                copy.contacts.iter_mut().for_each(|c| c.messages.clear());
                let app = copy.to_json().map_err(|e| e.to_string())?;
                let bytes = self
                    .core()
                    .without_sessions()
                    .save(&key, &app)
                    .map_err(|e| e.to_string())?;
                data::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
                self.data.backed_up = true;
                self.persist();
                Ok(format!(
                    "Backup written to {}.\nMove it off this device. Restore: Ctrl+B on the first-run screen.",
                    path.display()
                ))
            }
            Kind::Duress => {
                let (current, duress) = (f.fields[0].as_str(), f.fields[1].as_str());
                let file = std::fs::read(&self.path).map_err(|e| e.to_string())?;
                if store::open(current.as_bytes(), &file).is_err() {
                    return Err("current password is wrong".into());
                }
                if duress.is_empty() {
                    set_duress(&mut self.key, None).map_err(|e| e.to_string())?;
                    self.persist();
                    return Ok("Duress password turned off.".into());
                }
                if password::distance(current, duress) < DURESS_DISTANCE {
                    return Err(format!(
                        "must differ from your password by at least {DURESS_DISTANCE} characters"
                    ));
                }
                let duress = f.new_password(1, &[current, &self.data.nickname])?;
                set_duress(&mut self.key, Some(duress.as_bytes())).map_err(|e| e.to_string())?;
                self.persist();
                Ok("Duress password set. Entering it at unlock erases everything.".into())
            }
            Kind::Wipe => {
                if f.fields[0].trim() != "WIPE" {
                    return Err("type WIPE in capitals".into());
                }
                data::wipe(&self.path).map_err(|e| e.to_string())?;
                self.wiped = true;
                Ok(String::new())
            }
            Kind::Setup | Kind::Restore => Ok(String::new()),
        }
    }

    /// Adds a contact from an invite string and starts the handshake.
    fn add(&mut self, invite: &str) -> Result<(), String> {
        let invite = Invite::decode(invite).map_err(|e| format!("invalid invite: {e}"))?;
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
        let onion_matches =
            matches!((&invite.onion_url, &self.data.onion_url), (Some(a), Some(b)) if same(a, b));
        if !same(&invite.relay_url, &self.data.relay_url) && !onion_matches {
            return Err(format!(
                "they use another relay ({}); only one relay is supported for now",
                crate::ui::clean(&invite.relay_url)
            ));
        }
        let key = invite.identity.ed25519;
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
                    ..Default::default()
                });
                self.data.contacts.len() - 1
            }
        };
        self.select(i);
        self.persist();
        self.claim(key);
        Ok(())
    }
}

/// OSC 52: the terminal puts the text on the system clipboard.
fn copy_to_clipboard(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
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
        ] {
            assert!(!relay_url_ok(bad), "{bad}");
        }
    }
}
