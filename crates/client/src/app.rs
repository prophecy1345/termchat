//! TUI state and event handling, without any drawing.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use termchat_core::proto::{
    self, Body, ClientFrame, Ed25519Key, ErrorCode, Invite, ServerFrame, check_nickname,
};
use termchat_core::{
    Client, Error as CoreError, FileKey, KdfParams, create_file_key, set_duress, store,
};
use tokio::sync::mpsc::Receiver;
use tokio_tungstenite::tungstenite::http::Uri;

use crate::data::{self, Contact, Data, Msg, Status};
use crate::form::{self, Action, Form, Kind};
use crate::input::Input;
use crate::notify;
use crate::password;
use crate::settings::{self, Settings};
use crate::ui::Hits;
use crate::{Dial, Event, Relay, parse_version, theme};

/// Idle time before the client locks itself.
pub const AUTOLOCK: Duration = Duration::from_secs(5 * 60);
/// How long a wipe waits for the relay to take the goodbyes (ADR-009).
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);
const NOT_REMOVED: &str =
    "The relay did not confirm removing your key; it forgets idle keys after 90 days.";
/// ADR-007: what a not-yet-accepted request may leave behind.
const MAX_HIDDEN: usize = 5;
const MAX_REJECTED: usize = 1000;
/// ADR-003: a typo in the real password must never hit the duress one.
const DURESS_DISTANCE: usize = 4;
const RELEASES: &str = "https://github.com/prophecy1345/termchat/releases";

/// How this copy was installed decides how to update it (ADR-010).
fn update_command() -> Option<&'static str> {
    // Homebrew runs it through a symlink into its Cellar.
    let exe = std::env::current_exe().and_then(std::fs::canonicalize);
    let exe = exe.unwrap_or_default();
    let exe = exe.to_string_lossy();
    if exe.contains("/Cellar/") {
        Some("brew update && brew upgrade termchat")
    } else if cfg!(windows) {
        Some(
            "irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex",
        )
    } else if exe.ends_with(".local/bin/termchat") {
        Some(
            "curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh",
        )
    } else {
        None
    }
}

/// Disappearing timer steps for Ctrl+T.
const TIMERS: [Option<u32>; 5] = [None, Some(300), Some(3600), Some(86_400), Some(604_800)];

fn next_timer(now: Option<u32>) -> Option<u32> {
    let at = TIMERS.iter().position(|t| *t == now).unwrap_or(0);
    TIMERS[(at + 1) % TIMERS.len()]
}

/// What Ctrl+T does, e.g. "off → 5m" or "7d → off".
fn timer_step(now: Option<u32>) -> String {
    let name = |t: Option<u32>| t.map_or("off".into(), |s| crate::ui::span(s.into()));
    format!("{} → {}", name(now), name(next_timer(now)))
}

pub struct App {
    pub screen: Screen,
    pub path: PathBuf,
    pub settings: Settings,
    /// Where things were drawn last, for the mouse.
    pub hits: RefCell<Hits>,
    /// The contact list edge is being dragged.
    drag: bool,
    /// Whether the terminal window has focus; `None` until it says.
    pub focused: Option<bool>,
    /// New-message cards, oldest first.
    pub toasts: Vec<Toast>,
    last_sound: Option<Instant>,
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
    pub input: Input,
    pub error: String,
}

pub enum Popup {
    /// A long text with a filter: the keys or the manual.
    Pager {
        doc: Doc,
        filter: Input,
        /// Rows scrolled down from the top.
        scroll: usize,
    },
    Settings {
        tab: usize,
        at: usize,
    },
    Notice(String),
    /// A wipe is waiting for the relay (ADR-009).
    Leaving,
    /// The relay cannot be told; the reason, then "wipe anyway?".
    WipeAnyway(String),
    /// Ctrl+P, with the highlighted row.
    Menu(usize),
    /// Right click on a contact, with the highlighted row.
    Actions(usize),
    Form(Form),
    Add(Input),
    Invite(String),
    Verify,
    Rename(Input),
    Delete,
    /// The exact text that would be disclosed to the relay operator.
    Report(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doc {
    Keys,
    Manual,
    News,
}

/// Tabs of the settings window.
pub const TABS: [&str; 4] = ["theme", "notifications", "interface", "security"];
/// Mouse-only keys: a click on settings row `i` is `F(ROW_KEY + i)`, on tab
/// `i` it is `F(TAB_KEY + i)`. No keyboard has these.
pub const ROW_KEY: u8 = 100;
pub const TAB_KEY: u8 = 200;

/// A new-message card in the corner. Shows a name, never the text (ADR-008).
pub struct Toast {
    pub key: Ed25519Key,
    pub text: String,
    pub until: Instant,
}

const MAX_TOASTS: usize = 4;
const TOAST_FOR: Duration = Duration::from_secs(5);
/// A burst of messages makes one sound, not many.
const SOUND_GAP: Duration = Duration::from_secs(2);
/// A click on card `i` is `F(TOAST_KEY + i)`.
pub const TOAST_KEY: u8 = 150;

pub struct Main {
    pub core: Arc<Mutex<Client>>,
    key: FileKey,
    pub data: Data,
    relay: Relay,
    pub events: Receiver<Event>,
    pub connected: bool,
    /// A newer version the relay runs (ADR-010).
    pub update: Option<String>,
    pub selected: usize,
    /// History rows scrolled up from the bottom.
    pub scroll: usize,
    pub input: Input,
    pub popup: Option<Popup>,
    /// New messages (`false`) and requests (`true`) to tell the user about.
    pub alerts: Vec<(Ed25519Key, bool)>,
    pub status: String,
    pub last_input: Instant,
    path: PathBuf,
    /// Everything was erased; the app returns to the first-run screen with this note.
    wiped: Option<String>,
    leaving: Option<Leaving>,
}

/// Goodbyes sent and not yet accepted, then `Unregister` (ADR-009).
struct Leaving {
    waiting: Vec<u64>,
    until: Instant,
    unregistering: bool,
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
            false => {
                let _ = data::remove_tor_dir(&path);
                Screen::Setup(Form::new(Kind::Setup))
            }
        };
        Self {
            screen,
            settings: Settings::load(&settings::path(&path)),
            hits: RefCell::default(),
            drag: false,
            focused: None,
            toasts: Vec::new(),
            last_sound: None,
            path,
            quit: false,
        }
    }

    /// The terminal window title: an unread count, never a name (ADR-008).
    pub fn title(&self) -> String {
        let unread: usize = match &self.screen {
            Screen::Main(m) => m.data.contacts.iter().map(|c| c.unread).sum(),
            _ => 0,
        };
        match unread {
            0 => "termchat".into(),
            n => format!("termchat ({n})"),
        }
    }

    /// After a wipe the device must look freshly installed, theme included.
    fn wiped(&mut self, note: String) {
        self.settings = Settings::default();
        self.toasts.clear();
        let mut form = Form::new(Kind::Setup);
        form.error = note;
        self.screen = Screen::Setup(form);
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
        let Screen::Main(m) = &mut self.screen else {
            return;
        };
        m.on_relay(ev);
        if let Some(note) = m.wiped.take() {
            return self.wiped(note);
        }
        m.resort();
        for (key, request) in std::mem::take(&mut m.alerts) {
            // Already in front of the user's eyes.
            let open = m.current().is_some_and(|c| c.key == key);
            if open && self.focused != Some(false) {
                continue;
            }
            let Some(c) = m.index(&key).map(|i| &m.data.contacts[i]) else {
                continue;
            };
            let what = if request {
                "contact request"
            } else {
                "new message"
            };
            if self.toasts.len() >= MAX_TOASTS {
                self.toasts.remove(0);
            }
            self.toasts.push(Toast {
                key,
                text: format!("{what} · {}", crate::ui::name(m, c)),
                until: Instant::now() + TOAST_FOR,
            });
            let quiet = self.last_sound.is_some_and(|t| t.elapsed() < SOUND_GAP);
            if self.settings.sound && !quiet {
                self.last_sound = Some(Instant::now());
                notify::sound();
            }
            if self.settings.notify && self.focused != Some(true) {
                notify::system();
            }
        }
    }

    pub fn on_tick(&mut self) {
        self.toasts.retain(|t| t.until > Instant::now());
        if let Screen::Main(m) = &mut self.screen {
            if m.last_input.elapsed() >= AUTOLOCK {
                self.lock();
            } else {
                m.expire();
                m.leave_timeout();
                if let Some(note) = m.wiped.take() {
                    self.wiped(note);
                }
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
                Unlocked::Wiped => self.wiped(String::new()),
            },
            Screen::Main(m) => {
                m.last_input = Instant::now();
                let hits = self.hits.borrow().clone();
                let pager = matches!(m.popup, Some(Popup::Pager { .. }));
                let view = if pager { hits.pager } else { hits.history };
                let page = isize::try_from(view.height.max(2) - 1).unwrap_or(1);
                let by = match k.code {
                    KeyCode::PageUp => Some(page),
                    KeyCode::PageDown => Some(-page),
                    KeyCode::Up if pager => Some(1),
                    KeyCode::Down if pager => Some(-1),
                    _ => None,
                };
                if ctrl && k.code == KeyCode::Char('k') {
                    self.lock();
                } else if let KeyCode::F(f) = k.code
                    && (TOAST_KEY..TOAST_KEY + 10).contains(&f)
                {
                    // A click on a card opens that chat.
                    let i = usize::from(f - TOAST_KEY);
                    if i < self.toasts.len() {
                        let toast = self.toasts.remove(i);
                        if let Some(at) = m.index(&toast.key) {
                            m.select(at);
                        }
                    }
                } else if let Some(by) = by.filter(|_| pager || m.popup.is_none()) {
                    scroll(m, by, &hits);
                } else {
                    m.on_key(k, ctrl, &mut self.settings);
                    m.resort();
                    if let Some(note) = m.wiped.take() {
                        self.wiped(note);
                    }
                }
            }
        }
    }

    /// Clicks act like the key a button shows, so the mouse adds no new paths.
    pub fn on_mouse(&mut self, e: MouseEvent) {
        let at = Position::new(e.column, e.row);
        let hits = self.hits.borrow().clone();
        if let Screen::Main(m) = &mut self.screen {
            m.last_input = Instant::now();
        }
        let left = MouseEventKind::Down(MouseButton::Left);
        if e.kind == left
            && let Some((_, k)) = hits.keys.iter().find(|(r, _)| r.contains(at))
        {
            return self.on_key(*k);
        }
        let Screen::Main(m) = &mut self.screen else {
            if hits
                .window
                .is_some_and(|w| e.kind == left && !w.contains(at))
            {
                self.on_key(KeyEvent::from(KeyCode::Esc));
            }
            return;
        };
        if let Some(w) = hits.window {
            match e.kind {
                MouseEventKind::ScrollUp => scroll(m, 3, &hits),
                MouseEventKind::ScrollDown => scroll(m, -3, &hits),
                _ if e.kind == left && !w.contains(at) => self.on_key(KeyEvent::from(KeyCode::Esc)),
                _ => {}
            }
            return;
        }
        let row = |r: ratatui::layout::Rect| hits.list_offset + usize::from(at.y - r.y);
        match e.kind {
            MouseEventKind::Down(MouseButton::Left) if hits.list.contains(at) => {
                m.select(row(hits.list))
            }
            MouseEventKind::Down(MouseButton::Right) if hits.list.contains(at) => {
                m.select(row(hits.list));
                if m.current().is_some() {
                    m.popup = Some(Popup::Actions(0));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if hits.edge == Some(at.x) => self.drag = true,
            MouseEventKind::Drag(MouseButton::Left) if self.drag => {
                self.settings.sidebar = (e.column + 1).clamp(18, 40);
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag => {
                self.drag = false;
                if let Err(e) = self.settings.save(&settings::path(&self.path)) {
                    m.status = format!("cannot save settings: {e}");
                }
            }
            MouseEventKind::ScrollUp if hits.list.contains(at) => {
                m.select(m.selected.saturating_sub(1))
            }
            MouseEventKind::ScrollDown if hits.list.contains(at) => m.select(m.selected + 1),
            MouseEventKind::ScrollUp => scroll(m, 3, &hits),
            MouseEventKind::ScrollDown => scroll(m, -3, &hits),
            _ => {}
        }
    }

    /// Bracketed paste: the text goes into the active field as text only.
    /// Control characters are dropped so a pasted string cannot act as keys.
    pub fn on_paste(&mut self, text: &str) {
        let plain: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.screen {
            Screen::Setup(f) => f.paste(&plain),
            Screen::Unlock(u) => u.input.insert(&plain),
            Screen::Main(m) => {
                m.last_input = Instant::now();
                match &mut m.popup {
                    Some(Popup::Add(s) | Popup::Rename(s)) => s.insert(&plain),
                    Some(Popup::Form(f)) => f.paste(&plain),
                    Some(_) => {}
                    None => m.input.insert(&crate::ui::clean(text)),
                }
            }
        }
    }

    /// Saves and drops every key and message from memory.
    pub fn lock(&mut self) {
        if let Screen::Main(m) = &mut self.screen {
            m.persist();
        }
        // They carry names.
        self.toasts.clear();
        self.screen = Screen::Unlock(Unlock::default());
    }
}

fn setup(f: &Form, path: &Path) -> Result<Box<Main>, String> {
    let url = f.fields[0].trim();
    if !relay_url_ok(url) {
        return Err("relay URL must start with wss:// (ws:// only for localhost or .onion)".into());
    }
    let onion = f.fields[1].trim();
    let onion = (!onion.is_empty()).then(|| onion.to_string());
    if onion.as_ref().is_some_and(|o| {
        !(o.starts_with("ws://") && ws_host(o).is_some_and(|h| h.ends_with(".onion")))
    }) {
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
        news_seen: env!("CARGO_PKG_VERSION").into(),
        ..Data::default()
    };
    let mut main = Main::open(Client::default(), key, data, path.to_owned())?;
    if !main.persist() {
        return Err(main.status);
    }
    main.popup = Some(Popup::Notice(
        "Welcome. Click a contact to open the chat, right-click it for actions;\nCtrl+P opens the menu (settings, keys, manual), ? lists the keys.\n\nFirst, back up your identity: Ctrl+P, then b.\nWithout a backup, a lost device means a lost identity.".into(),
    ));
    Ok(main)
}

/// Plain `ws://` would show the relay traffic (and so who we talk to) to the
/// network; it is only fine where TLS adds nothing: the same machine or an
/// onion service, which authenticates and encrypts on its own.
fn relay_url_ok(url: &str) -> bool {
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

fn why(code: ErrorCode) -> String {
    match code {
        ErrorCode::UnknownRecipient => "contact is no longer on the relay".into(),
        code => format!("{code:?}"),
    }
}

/// A backup is a state file without sessions or messages: restoring is a copy.
fn restore(f: &Form, path: &Path) -> Result<(), String> {
    let backup = form::expand(&f.fields[0]);
    let bytes = std::fs::read(&backup).map_err(|e| format!("cannot read backup: {e}"))?;
    data::write_atomic(path, &bytes).map_err(|e| e.to_string())?;
    // ADR-003: the copy brought onto this device is erased with it.
    data::remember_backup(path, &backup, &bytes).map_err(|e| e.to_string())
}

pub enum Unlocked {
    Stay,
    Open(Box<Main>),
    /// The duress password was entered and the data is gone.
    Wiped,
}

impl Unlock {
    fn on_key(&mut self, k: KeyEvent, path: &Path) -> Unlocked {
        if !self.input.on_key(k) {
            return Unlocked::Stay;
        }
        // Argon2 blocks the UI thread for ~1 s; move to spawn_blocking if it bothers.
        let loaded = std::fs::read(path)
            .map_err(|e| format!("cannot read state: {e}"))
            .map(|bytes| Client::load(self.input.as_bytes(), &bytes));
        self.input = Input::default();
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
        (Some(onion), true) => (onion.as_str(), Dial::Tor(data::tor_dir(path))),
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
        let mut main = Box::new(Self {
            core,
            key,
            data,
            relay,
            events,
            connected: false,
            update: None,
            selected: 0,
            scroll: 0,
            input: Input::default(),
            popup: data::permission_warning(&path).map(Popup::Notice),
            alerts: Vec::new(),
            status: String::new(),
            last_input: Instant::now(),
            path,
            wiped: None,
            leaving: None,
        });
        main.resort();
        Ok(main)
    }

    /// Most recent first; the open chat stays open while rows move.
    fn resort(&mut self) {
        let open = self.current().map(|c| c.key);
        self.data
            .contacts
            .sort_by_key(|c| std::cmp::Reverse(c.last_at()));
        if let Some(i) = open.and_then(|k| self.index(&k)) {
            self.selected = i;
        }
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
                    self.popup = self.finish_wipe(NOT_REMOVED);
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
            } => self.status = format!("relay: {}", why(code)),
            ServerFrame::Unregistered => self.popup = self.finish_wipe(""),
            ServerFrame::Reported => {
                self.status = "reported to the relay operator, and blocked".into()
            }
            ServerFrame::Challenge { .. } | ServerFrame::AuthOk => {}
        }
    }

    /// ADR-009: goodbyes to every contact we talk to, then `Unregister`, then the wipe.
    /// `MOVED` (section 6) only erases this device, silently.
    fn leave(&mut self, mut f: Form) -> Option<Popup> {
        match f.fields[0].trim() {
            "WIPE" => {}
            "MOVED" => return self.finish_wipe(""),
            _ => {
                f.error = "type WIPE or MOVED in capitals".into();
                return Some(Popup::Form(f));
            }
        }
        if !self.connected {
            return Some(Popup::WipeAnyway("No connection to the relay.".into()));
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
        Some(Popup::Leaving)
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
        self.popup = match l.unregistering {
            true => self.finish_wipe(NOT_REMOVED),
            false => {
                self.leaving = None;
                Some(Popup::WipeAnyway(
                    "The relay did not answer in time; some contacts may already know.".into(),
                ))
            }
        };
    }

    /// Erases the device; `note` goes on the first-run screen. `Some` popup if it failed.
    fn finish_wipe(&mut self, note: &str) -> Option<Popup> {
        self.leaving = None;
        match data::wipe(&self.path) {
            Ok(()) => {
                self.wiped = Some(note.into());
                None
            }
            Err(e) => Some(Popup::Notice(format!("wipe failed: {e}"))),
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
                self.alerts.push((sender, false));
            }
            Body::ContactRequest { note, .. } => {
                let request = self.core().is_request(&sender);
                if request == Some(true) {
                    self.alerts.push((sender, true));
                }
                let text = match request {
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
            Body::Goodbye if got.request => {}
            Body::Goodbye => {
                self.system(i, "deleted their account; this chat is read-only now");
                self.data.contacts[i].left = true;
                self.alerts.push((sender, false));
            }
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

    fn on_key(&mut self, k: KeyEvent, ctrl: bool, settings: &mut Settings) {
        // Seen by now; a new problem sets it again.
        self.status.clear();
        if let Some(popup) = self.popup.take() {
            return self.on_popup_key(popup, k, settings);
        }
        let key = self.current().map(|c| c.key);
        match (ctrl, k.code) {
            (false, KeyCode::F(1)) => self.popup = Some(pager(Doc::Keys)),
            (false, KeyCode::Char('?')) if self.input.is_empty() => {
                self.popup = Some(pager(Doc::Keys))
            }
            (false, KeyCode::Up) => self.select(self.selected.saturating_sub(1)),
            (false, KeyCode::Down) => self.select(self.selected + 1),
            (true, KeyCode::Char('a')) => self.popup = Some(Popup::Add(Input::default())),
            (true, KeyCode::Char('p')) => self.popup = Some(Popup::Menu(0)),
            (true, KeyCode::Char('o')) => self.popup = Some(Popup::Invite(self.invite())),
            (true, KeyCode::Char('f')) if key.is_some() => self.popup = Some(Popup::Verify),
            (true, KeyCode::Char('r')) if key.is_some() => {
                self.popup = Some(Popup::Rename(Input::default()))
            }
            (true, KeyCode::Char('d')) if key.is_some() => self.popup = Some(Popup::Delete),
            (true, KeyCode::Char('y')) => {
                if let Some(key) = key
                    && self.core().accept(&key)
                {
                    let body = Body::ContactAccept {
                        nickname: self.data.nickname.clone(),
                    };
                    let i = self.selected;
                    let c = &mut self.data.contacts[i];
                    let hidden = std::mem::take(&mut c.hidden);
                    let held = std::mem::take(&mut c.held_back);
                    c.messages.extend(hidden);
                    if held > 0 {
                        self.system(
                            i,
                            format!(
                                "{held} more message(s) from before you accepted were not kept"
                            ),
                        );
                    }
                    match self.transmit(&key, body) {
                        Ok(_) => self.system(i, "you accepted the request"),
                        Err(e) => self.status = e,
                    }
                }
            }
            (true, KeyCode::Char('t')) => {
                if let Some(c) = self.data.contacts.get_mut(self.selected) {
                    c.expires_in = next_timer(c.expires_in);
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
            _ if self.input.on_key(k) => self.send(),
            _ => {}
        }
    }

    fn select(&mut self, i: usize) {
        if i < self.data.contacts.len() {
            self.selected = i;
            self.scroll = 0;
            self.data.contacts[i].unread = 0;
        }
    }

    /// This version's notes were not opened yet, or a newer one is out.
    pub fn news_unread(&self) -> bool {
        self.update.is_some() || self.data.news_seen != env!("CARGO_PKG_VERSION")
    }

    /// Top of "what's new" when the relay runs a newer version (ADR-010).
    pub fn update_note(&self) -> Option<String> {
        let v = self.update.as_ref()?;
        let how = match update_command() {
            Some(cmd) => format!("To update, quit termchat and run:\n\n```\n{cmd}\n```"),
            None => format!("Download it from {RELEASES}"),
        };
        Some(format!(
            "## termchat {v} is out\n\n{how}\n\nWhat changed: {RELEASES}\n\n"
        ))
    }

    /// Ctrl+P menu: hotkey and label.
    pub fn menu(&self) -> Vec<(char, String)> {
        let nag = if self.data.backed_up { "" } else { "  ●" };
        vec![
            ('s', "settings".into()),
            ('k', "keys".into()),
            ('m', "manual".into()),
            (
                'n',
                format!("what's new{}", if self.news_unread() { "  ●" } else { "" }),
            ),
            ('o', "your invite".into()),
            ('a', "add contact".into()),
            ('b', format!("back up your identity{nag}")),
        ]
    }

    /// Rows of a settings tab: label and whether it is ticked.
    pub fn settings_rows(&self, s: &Settings, tab: usize) -> Vec<(String, bool)> {
        let row = |label: &str, on: bool| (label.to_string(), on);
        match tab {
            0 => theme::THEMES
                .iter()
                .map(|(name, _)| row(name, *name == s.theme))
                .collect(),
            1 => vec![
                row("sound for new messages", s.sound),
                row("system notification: \"termchat: new message\"", s.notify),
            ],
            2 => vec![row("mouse (off: the terminal selects text)", s.mouse)],
            _ => vec![
                row(
                    &format!(
                        "back up your identity{}",
                        if self.data.backed_up { "" } else { "  ●" }
                    ),
                    false,
                ),
                row("change password", false),
                row("duress password", false),
                row("Tor", self.data.tor),
                row("wipe everything from this device", false),
            ],
        }
    }

    /// Settings window keys: Tab/arrows move, Enter or a click acts.
    fn on_settings_key(
        &mut self,
        tab: usize,
        at: usize,
        k: KeyEvent,
        s: &mut Settings,
    ) -> Option<Popup> {
        let n = self.settings_rows(s, tab).len().max(1);
        let theme_at = theme::index(&s.theme);
        let goto = |tab: usize| {
            let tab = tab % TABS.len();
            (tab, if tab == 0 { theme_at } else { 0 })
        };
        let (tab, at) = match k.code {
            KeyCode::Esc => return None,
            KeyCode::Tab | KeyCode::Right => goto(tab + 1),
            KeyCode::BackTab | KeyCode::Left => goto(tab + TABS.len() - 1),
            KeyCode::F(f) if f >= TAB_KEY => goto(usize::from(f - TAB_KEY)),
            KeyCode::F(f) if f >= ROW_KEY => {
                return self.settings_act(tab, usize::from(f - ROW_KEY), s);
            }
            KeyCode::Enter => return self.settings_act(tab, at, s),
            // Moving through themes shows each one at once.
            KeyCode::Up if tab == 0 => return self.settings_act(0, (at + n - 1) % n, s),
            KeyCode::Down if tab == 0 => return self.settings_act(0, (at + 1) % n, s),
            KeyCode::Up => (tab, (at + n - 1) % n),
            KeyCode::Down => (tab, (at + 1) % n),
            _ => (tab, at),
        };
        Some(Popup::Settings { tab, at })
    }

    fn settings_act(&mut self, tab: usize, at: usize, s: &mut Settings) -> Option<Popup> {
        match (tab, at) {
            (3, 0) => return Some(Popup::Form(Form::new(Kind::Backup))),
            (3, 1) => return Some(Popup::Form(Form::new(Kind::Password))),
            (3, 2) => return Some(Popup::Form(Form::new(Kind::Duress))),
            (3, 3) => return Some(Popup::Notice(self.toggle_tor())),
            (3, _) => return Some(Popup::Form(Form::new(Kind::Wipe))),
            (0, _) => {
                s.theme = theme::THEMES
                    .get(at)
                    .map_or(s.theme.clone(), |t| t.0.into())
            }
            (1, 0) => s.sound = !s.sound,
            (1, _) => s.notify = !s.notify,
            _ => s.mouse = !s.mouse,
        }
        if let Err(e) = s.save(&settings::path(&self.path)) {
            self.status = format!("cannot save settings: {e}");
        }
        Some(Popup::Settings { tab, at })
    }

    /// Right-click menu for the open contact: the Ctrl+key and its label.
    pub fn actions(&self) -> Vec<(char, String)> {
        let Some(c) = self.current() else {
            return Vec::new();
        };
        let mut items = Vec::new();
        if self.core().is_request(&c.key) == Some(true) {
            items.push(('y', "accept request".to_string()));
        }
        let verified = if c.verified {
            "verified"
        } else {
            "not verified"
        };
        items.extend([
            ('r', "rename".into()),
            ('f', format!("safety number ({verified})")),
            ('t', format!("disappearing: {}", timer_step(c.expires_in))),
            (
                'e',
                format!("no history: {}", if c.no_history { "on" } else { "off" }),
            ),
            ('b', if c.blocked { "unblock" } else { "block" }.into()),
            ('x', "report to the relay operator".into()),
            ('d', "delete".into()),
        ]);
        items
    }

    fn send(&mut self) {
        self.scroll = 0;
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
        if c.left {
            self.input = text;
            return self.status = "this contact left".into();
        }
        let (key, expires_in, i) = (c.key, c.expires_in, self.selected);
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
    }

    fn on_popup_key(&mut self, popup: Popup, k: KeyEvent, settings: &mut Settings) {
        let keep = match popup {
            Popup::Notice(_) => None,
            Popup::Leaving => Some(Popup::Leaving),
            Popup::WipeAnyway(_) => match k.code {
                KeyCode::Char('y') => self.finish_wipe(""),
                _ => None,
            },
            Popup::Pager { .. } if k.code == KeyCode::Esc => None,
            Popup::Pager {
                doc, mut filter, ..
            } => {
                filter.on_key(k);
                Some(Popup::Pager {
                    doc,
                    filter,
                    scroll: 0,
                })
            }
            Popup::Settings { tab, at } => self.on_settings_key(tab, at, k, settings),
            Popup::Menu(at) => match pick(&self.menu(), at, k) {
                Err(at) => Some(Popup::Menu(at)),
                Ok(None) => None,
                Ok(Some('s')) => {
                    let at = theme::index(&settings.theme);
                    Some(Popup::Settings { tab: 0, at })
                }
                Ok(Some('k')) => Some(pager(Doc::Keys)),
                Ok(Some('m')) => Some(pager(Doc::Manual)),
                Ok(Some('n')) => {
                    self.data.news_seen = env!("CARGO_PKG_VERSION").into();
                    self.persist();
                    Some(pager(Doc::News))
                }
                Ok(Some('b')) => Some(Popup::Form(Form::new(Kind::Backup))),
                // Invite and add: the same as Ctrl+O and Ctrl+A.
                Ok(Some(c)) => {
                    return self.on_key(
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                        true,
                        settings,
                    );
                }
            },
            Popup::Actions(at) => match pick(&self.actions(), at, k) {
                Err(at) => Some(Popup::Actions(at)),
                Ok(None) => None,
                // The same as pressing Ctrl+key in the chat.
                Ok(Some(c)) => {
                    return self.on_key(
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                        true,
                        settings,
                    );
                }
            },
            Popup::Form(mut f) => match f.on_key(k) {
                Action::Stay => Some(Popup::Form(f)),
                Action::Cancel => None,
                Action::Submit if f.kind == Kind::Wipe => self.leave(f),
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
                    // Turning down a request is remembered, so it cannot simply ask again.
                    let was_request = self.core().is_request(&c.key) == Some(true);
                    if was_request && !self.data.rejected.contains(&c.key) {
                        if self.data.rejected.len() >= MAX_REJECTED {
                            self.data.rejected.remove(0);
                        }
                        self.data.rejected.push(c.key);
                    }
                    self.core().remove_contact(&c.key);
                    self.selected = self.selected.saturating_sub(1);
                    self.persist();
                }
                None
            }
            Popup::Add(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    if let Err(e) = self.add(&s) {
                        self.status = e;
                    }
                    None
                }
                false => Some(Popup::Add(s)),
            },
            Popup::Rename(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
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
                if f.fields[0].trim().is_empty() {
                    return Err("where should it go? A USB stick, for example".into());
                }
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
                data::remember_backup(&self.path, &path, &bytes)
                    .map_err(|e| format!("backup written, but a wipe will not know it: {e}"))?;
                Ok(format!(
                    "Backup written to {}.\nMove it off this device: a wipe erases it only while it\nstays at this path. Restore: Ctrl+B on the first-run screen.",
                    path.display()
                ))
            }
            Kind::Duress => {
                let (current, duress) = (&*f.fields[0], &*f.fields[1]);
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
            Kind::Password => {
                let current = &*f.fields[0];
                let file = std::fs::read(&self.path).map_err(|e| e.to_string())?;
                if store::open(current.as_bytes(), &file).is_err() {
                    return Err("current password is wrong".into());
                }
                let new = f.new_password(1, &[current, &self.data.nickname])?;
                if new == current {
                    return Err("that is the current password".into());
                }
                // The duress password itself is unknown here, only its slot.
                if termchat_core::opens_duress(&self.key, new.as_bytes())
                    .map_err(|e| e.to_string())?
                {
                    return Err("that is your duress password".into());
                }
                termchat_core::rekey(&mut self.key, new.as_bytes()).map_err(|e| e.to_string())?;
                if !self.persist() {
                    return Err(self.status.clone());
                }
                Ok("Password changed.".into())
            }
            Kind::Setup | Kind::Restore | Kind::Wipe => Ok(String::new()),
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
        self.select(i);
        self.persist();
        self.claim(key);
        Ok(())
    }
}

/// Scrolls the open pager, or else the chat: `by` rows up (positive) or down.
fn scroll(m: &mut Main, by: isize, hits: &Hits) {
    match &mut m.popup {
        Some(Popup::Pager { scroll, .. }) => {
            *scroll = scroll.saturating_add_signed(-by).min(hits.pager_max);
        }
        None => m.scroll = m.scroll.saturating_add_signed(by).min(hits.max_scroll),
        Some(_) => {}
    }
}

fn pager(doc: Doc) -> Popup {
    Popup::Pager {
        doc,
        filter: Input::default(),
        scroll: 0,
    }
}

/// A menu: Up/Down move, Enter or a hotkey picks, any other key closes.
/// `Err` keeps it open with the new highlighted row.
fn pick(items: &[(char, String)], at: usize, k: KeyEvent) -> Result<Option<char>, usize> {
    let n = items.len().max(1);
    match k.code {
        KeyCode::Up => Err((at + n - 1) % n),
        KeyCode::Down => Err((at + 1) % n),
        KeyCode::Enter => Ok(items.get(at).map(|i| i.0)),
        KeyCode::Char(c) => Ok(items.iter().any(|i| i.0 == c).then_some(c)),
        _ => Ok(None),
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
    fn timer_step_names_the_next_press() {
        assert_eq!(timer_step(None), "off → 5m");
        assert_eq!(timer_step(Some(3600)), "1h → 1d");
        assert_eq!(timer_step(Some(604_800)), "7d → off");
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
