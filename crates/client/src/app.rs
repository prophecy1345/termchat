//! TUI state and event handling, without any drawing.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use termchat_core::proto::{Ed25519Key, REACTIONS, Ref};

use crate::data::{self, Contact, Group, Msg};
use crate::engine::{self, Engine, Happened, Setup, Unlocked, next_timer};
use crate::form::{self, Action, Form, Kind};
use crate::input::Input;
use crate::notify;
use crate::settings::{self, Idle, Settings};
use crate::ui::Hits;
use crate::{RelayEvent, theme};
use termchat_engine::NEW_DEVICE_NICKNAME;
use termchat_engine::notify::SOUND_GAP;

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
    /// The invite shown and its kind (ADR-013, ADR-014).
    Invite {
        text: String,
        kind: Share,
    },
    /// The nickname for a new incognito identity.
    Incognito(Input),
    /// Searching the open chat's history, on this device only.
    Search(Input),
    /// Tor bridge lines being edited, and the line being typed (ADR-015).
    Bridges {
        lines: Vec<String>,
        input: Input,
        error: String,
    },
    Verify,
    /// ADR-037: the safety number as a QR code, for their phone to scan.
    VerifyCode,
    Rename(Input),
    Delete,
    /// The exact text that would be disclosed to the relay operator.
    Report(String),
    /// Delete our message at this index for both sides (ADR-025)?
    Unsend(usize),
    /// ADR-027: the name of a new group.
    NewGroup(Input),
    /// The open group's members, with the highlighted row.
    Members(usize),
    /// Contacts the admin can invite, with the highlighted row.
    InviteToGroup(usize),
    /// Message `at` of the open chat goes to the chosen chat or group.
    Forward {
        at: usize,
        to: usize,
    },
    /// Remove this member from the open group?
    RemoveMember(Ed25519Key),
    /// Leave the open group?
    LeaveGroup,
    /// ADR-028: this device's link code, to paste on the main device.
    LinkCode(String),
    /// Our devices, with the highlighted row.
    Devices(usize),
    /// A new device's link code being pasted.
    AddDevice(Input),
    /// The fingerprint to compare on both screens, for own device `i`.
    Link(usize),
    /// Remove this device?
    RemoveDevice(Ed25519Key),
    /// A device's new label, seen only by our own devices.
    RenameDevice(Ed25519Key, Input),
}

/// What Enter does with the typed text (ADR-025).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Compose {
    #[default]
    New,
    /// An answer to this message; `theirs`: the contact's.
    Reply(Ref),
    /// New text for our message with this id.
    Edit(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// Works once.
    Once,
    /// Reusable until renewed.
    Public,
    /// Works once, from a new identity of its own.
    Incognito,
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
/// A click on card `i` is `F(TOAST_KEY + i)`.
pub const TOAST_KEY: u8 = 150;

pub struct Main {
    pub engine: Box<Engine>,
    pub selected: usize,
    /// History rows scrolled up from the bottom.
    pub scroll: usize,
    pub input: Input,
    pub popup: Option<Popup>,
    /// New messages (`false`) and requests (`true`) to tell the user about.
    pub alerts: Vec<(Ed25519Key, bool)>,
    pub status: String,
    /// The message picked with Shift+Up or Ctrl+N, by index (ADR-025).
    pub picked: Option<usize>,
    pub compose: Compose,
    pub idle: Idle,
    path: PathBuf,
    /// Everything was erased; the app returns to the first-run screen with this note.
    wiped: Option<String>,
    /// Set up only to be linked: show the link code once the relay is reached.
    linking: bool,
}

pub use crate::engine::now;

/// Shown after "y the same" until the other screen agrees too.
const LINK_CONFIRMED: &str = "confirmed: linking finishes when the other screen agrees";

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
            Screen::Main(m) => m
                .engine
                .data
                .contacts
                .iter()
                .filter(|c| !c.own)
                .map(|c| c.unread)
                .sum(),
            _ => 0,
        };
        match unread {
            0 => "termchat.app".into(),
            n => format!("termchat.app ({n})"),
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

    /// Every frame handed to the relay was answered, or nothing is unlocked.
    pub fn all_answered(&self) -> bool {
        match &self.screen {
            Screen::Main(m) => m.engine.all_answered(),
            _ => true,
        }
    }

    /// Next relay event while unlocked; never resolves otherwise.
    pub async fn relay_event(&mut self) -> RelayEvent {
        if let Screen::Main(m) = &mut self.screen
            && let Some(ev) = m.engine.events.recv().await
        {
            return ev;
        }
        std::future::pending().await
    }

    pub fn on_relay(&mut self, ev: RelayEvent) {
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
            let Some(c) = m.engine.index(&key).map(|i| &m.engine.data.contacts[i]) else {
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
            if m.idle.elapsed() >= self.settings.lock_after() {
                self.lock();
            } else {
                m.engine.tick();
                m.absorb();
                if m.linking
                    && m.popup.is_none()
                    && let Ok(code) = m.engine.link_code()
                {
                    m.linking = false;
                    m.status.clear();
                    m.popup = Some(Popup::LinkCode(code));
                }
                if let Some(note) = m.wiped.take() {
                    self.wiped(note);
                }
            }
        }
    }

    /// Input in an unlocked app: `true` if it came too late, and the app locked.
    fn idle_lock(&mut self) -> bool {
        let limit = self.settings.lock_after();
        let late = match &mut self.screen {
            Screen::Main(m) => !m.idle.touch(limit),
            _ => false,
        };
        if late {
            self.lock();
        }
        late
    }

    pub fn on_key(&mut self, k: KeyEvent) {
        if self.idle_lock() {
            return;
        }
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
            Screen::Setup(f) if ctrl && k.code == KeyCode::Char('l') && f.kind == Kind::Setup => {
                match setup_for_link(f, &self.path) {
                    Ok(main) => self.screen = Screen::Main(main),
                    Err(e) => f.error = e,
                }
            }
            Screen::Setup(f) => match f.on_key(k) {
                Action::Cancel if f.kind == Kind::Restore => {
                    self.screen = Screen::Setup(Form::new(Kind::Setup))
                }
                Action::Submit if f.kind == Kind::Restore => {
                    match engine::restore(&self.path, &form::expand(&f.fields[0])) {
                        Ok(()) => {
                            let hint = "restored: enter the backup's password".to_string();
                            self.screen = Screen::Unlock(Unlock {
                                error: hint,
                                ..Unlock::default()
                            });
                        }
                        Err(e) => f.error = e,
                    }
                }
                Action::Submit => match setup(f, &self.path) {
                    Ok(main) => self.screen = Screen::Main(main),
                    Err(e) => f.error = e,
                },
                _ => {}
            },
            Screen::Unlock(u) => match u.on_key(k, &self.path) {
                Unlocking::Stay => {}
                Unlocking::Open(main) => self.screen = Screen::Main(main),
                Unlocking::Wiped => self.wiped(String::new()),
            },
            Screen::Main(m) => {
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
                        if let Some(at) = m.engine.index(&toast.key) {
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
        if self.idle_lock() {
            return;
        }
        let at = Position::new(e.column, e.row);
        let hits = self.hits.borrow().clone();
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
        if self.idle_lock() {
            return;
        }
        let plain: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.screen {
            Screen::Setup(f) => f.paste(&plain),
            Screen::Unlock(u) => u.input.insert(&plain),
            Screen::Main(m) => {
                match &mut m.popup {
                    Some(
                        Popup::Add(s)
                        | Popup::Rename(s)
                        | Popup::Incognito(s)
                        | Popup::Search(s)
                        | Popup::NewGroup(s),
                    ) => s.insert(&plain),
                    // One bridge per pasted line; terminals often paste `\r` for a newline.
                    Some(Popup::Bridges { lines, .. }) => lines.extend(
                        text.split(['\r', '\n'])
                            .map(|l| l.chars().filter(|c| !c.is_control()).collect::<String>())
                            .map(|l| l.trim().to_string())
                            .filter(|l| !l.is_empty()),
                    ),
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

/// A device that will join an identity on another one (ADR-028): the same
/// keys and relay as a new identity, but no nickname to make up, and its
/// link code as soon as the relay is reached.
fn setup_for_link(f: &Form, path: &Path) -> Result<Box<Main>, String> {
    let setup = setup_of(f, NEW_DEVICE_NICKNAME)?;
    let mut main = Main::new(Engine::setup(path, &setup)?, path);
    main.linking = true;
    main.status = "reaching the relay for your link code…".into();
    Ok(main)
}

fn setup(f: &Form, path: &Path) -> Result<Box<Main>, String> {
    let setup = setup_of(f, f.fields[3].trim())?;
    let mut main = Main::new(Engine::setup(path, &setup)?, path);
    main.popup = Some(Popup::Notice(
        "Welcome. Click a contact to open the chat, right-click it for actions;\nCtrl+P opens the menu (settings, keys, manual), ? lists the keys.\n\nFirst, back up your identity: Ctrl+P, then b.\nWithout a backup, a lost device means a lost identity.".into(),
    ));
    Ok(main)
}

fn setup_of<'a>(f: &'a Form, nickname: &'a str) -> Result<Setup<'a>, String> {
    let tor = match f.fields[2].trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => return Err("Use Tor: answer y or n".into()),
    };
    let onion = f.fields[1].trim();
    let setup = Setup {
        relay_url: f.fields[0].trim(),
        onion_url: (!onion.is_empty()).then_some(onion),
        tor,
        nickname,
        password: f.new_password(4)?,
    };
    Ok(setup)
}

pub enum Unlocking {
    Stay,
    Open(Box<Main>),
    /// The duress password was entered and the data is gone.
    Wiped,
}

impl Unlock {
    fn on_key(&mut self, k: KeyEvent, path: &Path) -> Unlocking {
        if !self.input.on_key(k) {
            return Unlocking::Stay;
        }
        let result = Engine::unlock(path, self.input.as_bytes());
        self.input = Input::default();
        match result {
            Ok(Unlocked::Open(engine)) => Unlocking::Open(Main::new(engine, path)),
            Ok(Unlocked::Wiped) => Unlocking::Wiped,
            Err(e) => {
                self.error = e;
                Unlocking::Stay
            }
        }
    }
}

impl Main {
    fn new(engine: Box<Engine>, path: &Path) -> Box<Self> {
        let mut main = Box::new(Self {
            engine,
            selected: 0,
            scroll: 0,
            input: Input::default(),
            popup: None,
            alerts: Vec::new(),
            status: String::new(),
            picked: None,
            compose: Compose::New,
            idle: Idle::default(),
            path: path.to_owned(),
            wiped: None,
            linking: false,
        });
        main.absorb();
        main.resort();
        main.load_draft();
        main
    }

    fn public_invite(&mut self, renew: bool) -> Option<Popup> {
        match self.engine.public_invite(renew) {
            Ok(text) => Some(Popup::Invite {
                text,
                kind: Share::Public,
            }),
            Err(e) => {
                self.status = e;
                None
            }
        }
    }

    /// Takes in what the engine reported since the last call.
    fn absorb(&mut self) {
        // An open group is read as it comes.
        self.engine.open_group = self.current_group().map(|g| g.id);
        if let Some(g) = self.group_index() {
            self.engine.data.groups[g].unread = 0;
        }
        // The link is done or gone: the "confirmed, waiting" line has said its piece.
        if self.status.starts_with(LINK_CONFIRMED) && self.engine.pending_link().is_none() {
            self.status.clear();
        }
        // ADR-028: a device waiting for the fingerprint check asks at once,
        // over its own link code, which has done its job.
        if matches!(self.popup, None | Some(Popup::LinkCode(_)))
            && let Some(i) = self.engine.pending_link()
        {
            self.popup = Some(Popup::Link(i));
        }
        for h in std::mem::take(&mut self.engine.happened) {
            match h {
                Happened::Alert { key, request } => self.alerts.push((key, request)),
                Happened::Note(n) => self.status = n,
                Happened::Notice(n) => self.popup = Some(Popup::Notice(n)),
                // The list shows a group's unread count; no bell for groups yet.
                Happened::GroupAlert { .. } => {}
                Happened::Wiped(n) => self.wiped = Some(n),
                Happened::WipeAnyway(why) => self.popup = Some(Popup::WipeAnyway(why)),
                Happened::Invite(text) => {
                    self.popup = Some(Popup::Invite {
                        text,
                        kind: Share::Incognito,
                    })
                }
            }
        }
    }

    /// Most recent first; the open chat stays open while rows move.
    fn resort(&mut self) {
        let open = self.current().map(|c| c.key);
        let group = self.current_group().map(|g| g.id);
        self.engine.resort();
        if let Some(i) = open.and_then(|k| self.engine.index(&k)) {
            self.selected = i;
        }
        // A new contact pushes the groups down; the open one stays open.
        if let Some(g) =
            group.and_then(|id| self.engine.data.groups.iter().position(|g| g.id == id))
        {
            self.selected = self.engine.data.people() + g;
        }
    }

    fn persist(&mut self) {
        self.stash_draft();
        self.engine.persist();
        self.absorb();
    }

    /// The unsent text of the open chat or group, where its draft is kept.
    fn draft_slot(&mut self) -> Option<&mut String> {
        let n = self.engine.data.people();
        match self.selected.checked_sub(n) {
            None => self
                .engine
                .data
                .contacts
                .get_mut(self.selected)
                .map(|c| &mut c.draft),
            Some(g) => self.engine.data.groups.get_mut(g).map(|g| &mut g.draft),
        }
    }

    /// Text being typed becomes the open chat's draft; an edit of a sent
    /// message does not.
    fn stash_draft(&mut self) {
        let text = match self.compose {
            Compose::Edit(_) => String::new(),
            _ => self.input.to_string(),
        };
        if let Some(draft) = self.draft_slot() {
            *draft = text;
        }
    }

    fn load_draft(&mut self) {
        let text = self.draft_slot().map(|d| d.clone()).unwrap_or_default();
        self.input = Input::from(text.as_str());
    }

    fn on_relay(&mut self, ev: RelayEvent) {
        self.engine.open = self.current().map(|c| c.key);
        self.engine.on_relay(ev);
        // A first contact shows up already open.
        if let Some(c) = self.engine.data.contacts.get_mut(self.selected) {
            c.unread = 0;
        }
        self.absorb();
    }

    pub fn current(&self) -> Option<&Contact> {
        // Our own devices (ADR-028) come after the people and are never open.
        let people = &self.engine.data.contacts[..self.engine.data.people()];
        people.get(self.selected)
    }

    fn on_key(&mut self, k: KeyEvent, ctrl: bool, settings: &mut Settings) {
        // Seen by now; a new problem sets it again.
        self.status.clear();
        if let Some(popup) = self.popup.take() {
            self.on_popup_key(popup, k, settings);
            return self.absorb();
        }
        if let Some(at) = self.picked {
            self.on_pick_key(at, k);
            return self.absorb();
        }
        let key = self.current().map(|c| c.key);
        let i = self.selected;
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match (ctrl, k.code) {
            (true, KeyCode::Char('n')) | (false, KeyCode::Up)
                if (key.is_some() || self.group_index().is_some()) && (ctrl || shift) =>
            {
                match self.pickable().last() {
                    Some(&at) => self.picked = Some(at),
                    None => self.status = "no message here can be answered".into(),
                }
            }
            // ADR-027: in a group, these keys mean the group's own actions.
            (true, KeyCode::Char('t')) if self.group_index().is_some() => {
                if let Some(g) = self.group_index() {
                    let next = next_timer(self.engine.data.groups[g].timer());
                    if let Err(e) = self.engine.set_group_timer(g, next) {
                        self.status = e;
                    }
                }
            }
            (true, KeyCode::Char('d')) if self.group_index().is_some() => {
                self.popup = Some(Popup::LeaveGroup)
            }
            (true, KeyCode::Char('v')) => {
                let until = match self.group_index() {
                    Some(g) => {
                        let group = &mut self.engine.data.groups[g];
                        group.muted_until = engine::next_mute(group.muted_until);
                        let until = group.muted_until;
                        self.engine.persist();
                        until
                    }
                    None if self.current().is_some() => {
                        self.engine
                            .edit(i, |c| c.muted_until = engine::next_mute(c.muted_until));
                        self.current().and_then(|c| c.muted_until)
                    }
                    None => return,
                };
                self.status = format!("mute: {}", engine::mute_label(until));
            }
            (true, KeyCode::Char('r')) if self.group_index().is_some() => {
                self.popup = Some(Popup::Members(0))
            }
            (true, KeyCode::Char('e' | 'b' | 'x' | 'f' | 'y' | 's'))
                if self.group_index().is_some() =>
            {
                self.status = "not in a group: Ctrl+R for members, Ctrl+T timer, Ctrl+D leave, \
                     Ctrl+L send a file, Ctrl+G save one"
                    .into()
            }
            (false, KeyCode::Esc) if self.compose != Compose::New => {
                if matches!(self.compose, Compose::Edit(_)) {
                    self.input = Input::default();
                }
                self.compose = Compose::New;
            }
            (false, KeyCode::F(1)) => self.popup = Some(pager(Doc::Keys)),
            (false, KeyCode::Char('?')) if self.input.is_empty() => {
                self.popup = Some(pager(Doc::Keys))
            }
            (false, KeyCode::Up) => self.select(self.selected.saturating_sub(1)),
            (false, KeyCode::Down) => self.select(self.selected + 1),
            (true, KeyCode::Char('a')) => self.popup = Some(Popup::Add(Input::default())),
            (true, KeyCode::Char('p')) => self.popup = Some(Popup::Menu(0)),
            (true, KeyCode::Char('o')) => match self.engine.invite() {
                Ok(text) => {
                    self.popup = Some(Popup::Invite {
                        text,
                        kind: Share::Once,
                    })
                }
                Err(e) => self.status = e,
            },
            (true, KeyCode::Char('f')) if key.is_some() => self.popup = Some(Popup::Verify),
            (true, KeyCode::Char('s')) if key.is_some() => {
                self.popup = Some(Popup::Search(Input::default()))
            }
            (true, KeyCode::Char('r')) if key.is_some() => {
                self.popup = Some(Popup::Rename(Input::default()))
            }
            (true, KeyCode::Char('d')) if key.is_some() => self.popup = Some(Popup::Delete),
            (true, KeyCode::Char('y')) => {
                let invite = key.and_then(|k| {
                    self.engine
                        .data
                        .group_invites
                        .iter()
                        .position(|x| x.from == k)
                });
                match invite {
                    // Not a request but a group invite from this contact.
                    Some(n) if key.is_some_and(|k| !self.engine.is_request(&k)) => {
                        match self.engine.accept_group(n) {
                            Ok(g) => {
                                let at = self.engine.data.people() + g;
                                self.select(at);
                            }
                            Err(e) => self.status = e,
                        }
                    }
                    _ if key.is_some() => self.engine.accept(i),
                    _ => {}
                }
            }
            (true, KeyCode::Char('t')) => self
                .engine
                .edit(i, |c| c.expires_in = next_timer(c.expires_in)),
            (true, KeyCode::Char('x')) => {
                if let Some(text) = self.engine.report_text(i) {
                    self.popup = Some(Popup::Report(text));
                }
            }
            (true, KeyCode::Char('b')) => {
                let on = self.current().is_some_and(|c| !c.blocked);
                self.engine.block(i, on);
                if let Some(c) = self.current() {
                    self.status = match c.blocked {
                        true => "blocked: their queue is deleted, nothing of theirs arrives",
                        false => "unblocked",
                    }
                    .into();
                }
            }
            (true, KeyCode::Char('e')) => self.engine.edit(i, |c| c.no_history = !c.no_history),
            (true, KeyCode::Char('l')) if key.is_some() || self.group_index().is_some() => {
                self.popup = Some(Popup::Form(Form::new(Kind::SendFile)))
            }
            // Documents in a group (ADR-027 amendment).
            (true, KeyCode::Char('g')) if self.group_index().is_some() => {
                match self
                    .group_index()
                    .and_then(|g| self.engine.unsaved_group_file(g))
                {
                    Some(_) => self.popup = Some(Popup::Form(Form::new(Kind::SaveFile))),
                    None => self.status = "no unsaved file in this group".into(),
                }
            }
            (true, KeyCode::Char('g')) if key.is_some() => match self.engine.unsaved_file(i) {
                Some(_) => self.popup = Some(Popup::Form(Form::new(Kind::SaveFile))),
                None => self.status = "no unsaved file in this chat".into(),
            },
            _ if self.input.on_key(k) => self.send(),
            _ => {}
        }
        self.absorb();
    }

    /// The open group, if a group is open (ADR-027): groups follow the
    /// contacts in the list.
    pub fn group_index(&self) -> Option<usize> {
        let g = self.selected.checked_sub(self.engine.data.people())?;
        (g < self.engine.data.groups.len()).then_some(g)
    }

    pub fn current_group(&self) -> Option<&Group> {
        self.engine.data.groups.get(self.group_index()?)
    }

    /// The messages of the open chat or group.
    fn messages(&self) -> &[Msg] {
        match (self.current(), self.current_group()) {
            (Some(c), _) => &c.messages,
            (None, Some(g)) => &g.messages,
            _ => &[],
        }
    }

    /// Messages of the open chat that can be answered or reacted to.
    pub fn pickable(&self) -> Vec<usize> {
        let all = self.messages();
        (0..all.len())
            .filter(|&n| {
                let m = &all[n];
                m.id.is_some() && !m.deleted && m.status != crate::data::Status::System
            })
            .collect()
    }

    /// Where the message `r` points at is now, in the open chat. In a group
    /// the id alone names it.
    fn locate(&self, r: Ref) -> Option<usize> {
        let group = self.group_index().is_some();
        self.messages()
            .iter()
            .position(|m| m.id == Some(r.id) && (group || m.mine != r.theirs) && !m.deleted)
    }

    /// Keys while a message is picked (ADR-025).
    fn on_pick_key(&mut self, at: usize, k: KeyEvent) {
        let i = self.selected;
        let all = self.pickable();
        let Some(pos) = all.iter().position(|&n| n == at) else {
            self.picked = None;
            return;
        };
        let group = self.group_index();
        let Some(msg) = self.messages().get(at) else {
            self.picked = None;
            return;
        };
        let (mine, id) = (msg.mine, msg.id);
        let reaction = match (self.current_group(), id) {
            (Some(g), Some(id)) => {
                let me = self.engine.me();
                g.reactions_to(id).find(|(k, _)| *k == me).map(|(_, r)| r)
            }
            _ => msg.my_reaction,
        };
        // Every member of a group reads answers and reactions (ADR-027).
        let rich = group.is_some() || self.engine.rich(i);
        let target = id.map(|id| Ref { id, theirs: !mine });
        let not_ours = "only your own messages can be changed";
        match k.code {
            KeyCode::Up => self.picked = Some(all[pos.saturating_sub(1)]),
            KeyCode::Down => self.picked = all.get(pos + 1).copied(),
            KeyCode::Esc => self.picked = None,
            KeyCode::Char('r') | KeyCode::Enter => match rich {
                true => {
                    self.compose = target.map_or(Compose::New, Compose::Reply);
                    self.picked = None;
                }
                false => self.status = engine::OLDER_APP.into(),
            },
            KeyCode::Char('e') if !mine => self.status = not_ours.into(),
            KeyCode::Char('e') => match (rich, id) {
                (true, Some(id)) => {
                    self.compose = Compose::Edit(id);
                    let text = self.messages().get(at).map(|m| m.text.clone());
                    self.input = Input::default();
                    self.input.insert(&text.unwrap_or_default());
                    self.picked = None;
                }
                _ => self.status = engine::OLDER_APP.into(),
            },
            KeyCode::Char('f') => match self.messages().get(at).and_then(|m| m.forwardable()) {
                Some(_) => self.popup = Some(Popup::Forward { at, to: 0 }),
                None => {
                    self.status = "nothing here to forward: a saved document goes from disk".into()
                }
            },
            KeyCode::Char('d') if !mine => self.status = not_ours.into(),
            KeyCode::Char('d') => self.popup = Some(Popup::Unsend(at)),
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as u8 - b'1';
                if n < REACTIONS {
                    // The same one again takes it back.
                    let to = (reaction != Some(n)).then_some(n);
                    let done = match group {
                        Some(g) => self.engine.react_group(g, at, to),
                        None => self.engine.react(i, at, to),
                    };
                    if let Err(e) = done {
                        self.status = e;
                    }
                }
            }
            _ => {}
        }
    }

    fn select(&mut self, i: usize) {
        let n = self.engine.data.people();
        if i >= n + self.engine.data.groups.len() {
            return;
        }
        let moved = i != self.selected;
        if moved {
            self.stash_draft();
        }
        self.selected = i;
        self.scroll = 0;
        self.picked = None;
        self.compose = Compose::New;
        match i.checked_sub(n) {
            Some(g) => self.engine.data.groups[g].unread = 0,
            None => self.engine.data.contacts[i].unread = 0,
        }
        if moved {
            self.load_draft();
        }
    }

    /// This version's notes were not opened yet, or a newer one is out.
    pub fn news_unread(&self) -> bool {
        self.engine.news_unread()
    }

    /// Top of "what's new" when the relay runs a newer version (ADR-010).
    pub fn update_note(&self) -> Option<String> {
        let v = self.engine.update.as_ref()?;
        let how = match update_command() {
            Some(cmd) => format!("To update, quit termchat.app and run:\n\n```\n{cmd}\n```"),
            None => format!("Download it from {RELEASES}"),
        };
        Some(format!(
            "## termchat.app {v} is out\n\n{how}\n\nWhat changed: {RELEASES}\n\n"
        ))
    }

    /// Ctrl+P menu: hotkey and label.
    pub fn menu(&self) -> Vec<(char, String)> {
        let nag = if !self.engine.needs_backup() {
            ""
        } else {
            "  ●"
        };
        let mut rows = vec![
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
            ('g', "new group".into()),
        ];
        // ADR-028: a new device links itself; the main one manages them.
        let e = &self.engine;
        if e.is_primary() && e.data.devices.is_none() && e.data.people() == 0 {
            rows.push(('l', "link to my main device".into()));
        }
        if e.is_primary() || e.data.devices.is_some() {
            rows.push(('v', "devices".into()));
        }
        rows.extend(self.place_rows());
        if self.current().is_some() || self.group_index().is_some() {
            rows.push(('e', "export this chat to a text file".into()));
        }
        rows
    }

    /// Pin and archive for the open chat, in both menus.
    fn place_rows(&self) -> Vec<(char, String)> {
        let Some(c) = self.current() else {
            return Vec::new();
        };
        vec![
            (
                'i',
                if c.pinned {
                    "unpin this chat"
                } else {
                    "pin this chat"
                }
                .into(),
            ),
            (
                'z',
                if c.archived {
                    "unarchive this chat"
                } else {
                    "archive this chat"
                }
                .into(),
            ),
        ]
    }

    /// `i` pins or unpins the open chat, `z` archives or brings it back.
    fn place(&mut self, what: char) {
        let i = self.selected;
        match what {
            'i' => self.engine.edit(i, |c| c.pinned = !c.pinned),
            _ => self.engine.edit(i, |c| c.archived = !c.archived),
        }
        self.resort();
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
                row("delivery receipts: tell contacts it arrived", s.receipts),
                row("read receipts: send yours, see theirs", s.read_receipts),
            ],
            2 => vec![row("mouse (off: the terminal selects text)", s.mouse)],
            _ => vec![
                row(
                    &format!(
                        "back up your identity{}",
                        if !self.engine.needs_backup() {
                            ""
                        } else {
                            "  ●"
                        }
                    ),
                    false,
                ),
                row("change password", false),
                row("duress password", false),
                row("Tor", self.engine.data.tor),
                row(
                    &format!("Tor bridges ({})", self.engine.data.bridges.len()),
                    !self.engine.data.bridges.is_empty(),
                ),
                row(
                    "one connection for all contacts (less battery; the relay sees which queues are yours)",
                    self.engine.data.shared_link,
                ),
                row(
                    "connect the open chat at once (with Tor the relay may see which chat you open)",
                    self.engine.data.hurry_open,
                ),
                row(
                    &format!(
                        "lock after {} min idle (longer: riskier if left open)",
                        s.lock_minutes.clamp(1, 30)
                    ),
                    false,
                ),
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
            (3, 3) => return Some(Popup::Notice(self.engine.toggle_tor())),
            (3, 4) => {
                return Some(Popup::Bridges {
                    lines: self.engine.data.bridges.clone(),
                    input: Input::default(),
                    error: String::new(),
                });
            }
            (3, 5) => {
                let on = !self.engine.data.shared_link;
                return Some(Popup::Notice(match self.engine.set_shared_link(on) {
                    Ok(done) | Err(done) => done,
                }));
            }
            (3, 6) => {
                let on = !self.engine.data.hurry_open;
                self.engine.set_hurry_open(on);
            }
            (3, 7) => s.next_lock(),
            (3, _) => return Some(Popup::Form(Form::new(Kind::Wipe))),
            (0, _) => {
                s.theme = theme::THEMES
                    .get(at)
                    .map_or(s.theme.clone(), |t| t.0.into())
            }
            (1, 0) => s.sound = !s.sound,
            (1, 1) => s.notify = !s.notify,
            (1, 2) => s.receipts = !s.receipts,
            (1, _) => s.read_receipts = !s.read_receipts,
            _ => s.mouse = !s.mouse,
        }
        (self.engine.receipts, self.engine.read_receipts) = (s.receipts, s.read_receipts);
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
        if self.engine.is_request(&c.key) {
            items.push(('y', "accept request".to_string()));
        }
        let verified = if c.verified {
            "verified"
        } else {
            "not verified"
        };
        items.push(('l', "send a file".into()));
        if self.engine.unsaved_file(self.selected).is_some() {
            items.push(('g', "save the file".into()));
        }
        items.extend([
            ('r', "rename".into()),
            ('f', format!("safety number ({verified})")),
            ('t', format!("disappearing: {}", timer_step(c.expires_in))),
            (
                'e',
                format!("no history: {}", if c.no_history { "on" } else { "off" }),
            ),
            ('v', format!("mute: {}", engine::mute_label(c.muted_until))),
            ('b', if c.blocked { "unblock" } else { "block" }.into()),
            ('x', "report to the relay operator".into()),
            ('d', "delete".into()),
        ]);
        items.extend(self.place_rows());
        items
    }

    fn send(&mut self) {
        self.scroll = 0;
        let text = std::mem::take(&mut self.input);
        if text.trim().is_empty() && (self.current().is_some() || self.group_index().is_some()) {
            return;
        }
        let i = self.selected;
        let group = self.group_index();
        let gone = "the message you answer is gone; Esc to write without it";
        let sent = match (self.compose, group) {
            (Compose::New, Some(g)) => self.engine.send_group(g, &text),
            (Compose::New, None) => self.engine.send(i, &text),
            (Compose::Reply(r), g) => match (self.locate(r), g) {
                (Some(at), Some(g)) => self.engine.reply_group(g, at, &text),
                (Some(at), None) => self.engine.reply(i, at, &text),
                (None, _) => Err(gone.into()),
            },
            (Compose::Edit(id), g) => match (self.locate(Ref { id, theirs: false }), g) {
                (Some(at), Some(g)) => self.engine.edit_group_message(g, at, &text),
                (Some(at), None) => self.engine.edit_message(i, at, &text),
                (None, _) => Err("the message is gone; Esc to stop editing".into()),
            },
        };
        match sent {
            Ok(()) => {
                self.compose = Compose::New;
                if let Some(draft) = self.draft_slot() {
                    draft.clear();
                }
            }
            Err(e) => {
                self.input = text;
                self.status = e;
            }
        }
    }

    fn on_popup_key(&mut self, popup: Popup, k: KeyEvent, settings: &mut Settings) {
        let i = self.selected;
        let keep = match popup {
            Popup::Notice(_) => None,
            Popup::Leaving => Some(Popup::Leaving),
            Popup::WipeAnyway(_) => {
                if k.code == KeyCode::Char('y') {
                    self.engine.finish_wipe("");
                }
                None
            }
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
                    self.engine.news_seen();
                    Some(pager(Doc::News))
                }
                Ok(Some('b')) => Some(Popup::Form(Form::new(Kind::Backup))),
                Ok(Some('g')) => Some(Popup::NewGroup(Input::default())),
                Ok(Some('l')) => match self.engine.link_code() {
                    Ok(code) => Some(Popup::LinkCode(code)),
                    Err(e) => {
                        self.status = e;
                        None
                    }
                },
                Ok(Some('v')) => Some(Popup::Devices(0)),
                Ok(Some(c @ ('i' | 'z'))) => {
                    self.place(c);
                    None
                }
                Ok(Some('e')) => Some(Popup::Form(Form::new(Kind::Export))),
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
                Ok(Some(c @ ('i' | 'z'))) => {
                    self.place(c);
                    None
                }
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
                Action::Submit if f.kind == Kind::Wipe => match f.fields[0].trim() {
                    "WIPE" => self.engine.leave(false).then_some(Popup::Leaving),
                    "MOVED" => {
                        self.engine.leave(true);
                        None
                    }
                    _ => {
                        f.error = "type WIPE or MOVED in capitals".into();
                        Some(Popup::Form(f))
                    }
                },
                Action::Submit => match self.submit(&f) {
                    Ok(done) => Some(Popup::Notice(done)),
                    Err(e) => {
                        f.error = e;
                        Some(Popup::Form(f))
                    }
                },
            },
            Popup::Invite { text, kind } => match k.code {
                KeyCode::Char('c') => {
                    copy_to_clipboard(&text);
                    self.status = "invite copied (if your terminal supports OSC 52)".into();
                    None
                }
                KeyCode::Char('p') if kind == Share::Once => self.public_invite(false),
                KeyCode::Char('i') if kind == Share::Once => {
                    Some(Popup::Incognito(Input::default()))
                }
                KeyCode::Char('n') if kind == Share::Public => {
                    self.status = "new public invite: the old one no longer works".into();
                    self.public_invite(true)
                }
                KeyCode::Char('r') if kind == Share::Once => {
                    let n = self.engine.revoke_invites();
                    self.status = format!("{n} unused one-time invite(s) revoked");
                    None
                }
                _ => None,
            },
            Popup::Bridges {
                mut lines,
                mut input,
                mut error,
            } if k.code != KeyCode::Esc => {
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                match k.code {
                    KeyCode::Char('s') if ctrl => {
                        match self.engine.set_bridges(&lines.join("\n")) {
                            Ok(done) => return self.popup = Some(Popup::Notice(done)),
                            Err(e) => error = e,
                        }
                    }
                    KeyCode::Char('x') if ctrl => lines.clear(),
                    _ if input.on_key(k) && !input.trim().is_empty() => {
                        lines.push(input.take().trim().to_string());
                    }
                    _ => {}
                }
                Some(Popup::Bridges {
                    lines,
                    input,
                    error,
                })
            }
            // Results update as you type; Enter or Esc closes.
            Popup::Search(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => None,
                false => Some(Popup::Search(s)),
            },
            Popup::Incognito(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    // The invite pops up once its queue exists (ADR-023).
                    self.status = match self.engine.incognito_invite(&s) {
                        Ok(()) => "making a new identity; its invite appears in a moment".into(),
                        Err(e) => e,
                    };
                    None
                }
                false => Some(Popup::Incognito(s)),
            },
            Popup::Verify => match k.code {
                KeyCode::Char('v') => {
                    self.engine.edit(i, |c| c.verified = !c.verified);
                    None
                }
                KeyCode::Char('q') => Some(Popup::VerifyCode),
                _ => None,
            },
            Popup::VerifyCode => match k.code {
                KeyCode::Esc => None,
                _ => Some(Popup::Verify),
            },
            Popup::Report(text) => {
                if k.code == KeyCode::Char('y') {
                    self.engine.report(i, text);
                }
                None
            }
            Popup::Unsend(at) => {
                if k.code == KeyCode::Char('y')
                    && let Err(e) = self.engine.delete_message(i, at)
                {
                    self.status = e;
                }
                self.picked = None;
                None
            }
            Popup::Delete => {
                if k.code == KeyCode::Char('y') && i < self.engine.data.people() {
                    self.engine.delete(i);
                    self.selected = i.saturating_sub(1);
                }
                None
            }
            // Ctrl+N: from a new identity of its own (ADR-014).
            Popup::Add(s)
                if k.code == KeyCode::Char('n') && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                match self.engine.add(&s, true) {
                    Ok(i) => self.select(i),
                    Err(e) => self.status = e,
                }
                None
            }
            Popup::Add(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    match self.engine.add(&s, false) {
                        Ok(i) => self.select(i),
                        Err(e) => self.status = e,
                    }
                    None
                }
                false => Some(Popup::Add(s)),
            },
            Popup::NewGroup(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => match self.engine.create_group(&s) {
                    Ok(g) => {
                        let at = self.engine.data.people() + g;
                        self.select(at);
                        None
                    }
                    Err(e) => {
                        self.status = e;
                        Some(Popup::NewGroup(s))
                    }
                },
                false => Some(Popup::NewGroup(s)),
            },
            Popup::Members(at) => {
                let Some(g) = self.group_index() else { return };
                let n = self.engine.data.groups[g].members().len();
                let admin = self.engine.data.groups[g].is_admin();
                match k.code {
                    KeyCode::Up => Some(Popup::Members(at.saturating_sub(1))),
                    KeyCode::Down => Some(Popup::Members((at + 1).min(n.saturating_sub(1)))),
                    KeyCode::Char('a') if admin => Some(Popup::InviteToGroup(0)),
                    KeyCode::Char('x') if admin => {
                        let who = self.engine.data.groups[g]
                            .members()
                            .get(at)
                            .map(|m| m.ed25519);
                        who.filter(|k| *k != self.engine.me())
                            .map(Popup::RemoveMember)
                    }
                    KeyCode::Esc => None,
                    _ => Some(Popup::Members(at)),
                }
            }
            Popup::Forward { at, to } => {
                let n = self.engine.data.people();
                let last = (n + self.engine.data.groups.len()).saturating_sub(1);
                match k.code {
                    KeyCode::Up => Some(Popup::Forward {
                        at,
                        to: to.saturating_sub(1),
                    }),
                    KeyCode::Down => Some(Popup::Forward {
                        at,
                        to: (to + 1).min(last),
                    }),
                    KeyCode::Enter => {
                        let what = self.messages().get(at).and_then(|m| m.forwardable());
                        let sent = match (what, to.checked_sub(n)) {
                            (None, _) => Err("the message is gone".to_string()),
                            (Some(w), Some(g)) => self.engine.forward_group(g, w),
                            (Some(w), None) => self.engine.forward(to, w),
                        };
                        self.status = match sent {
                            Ok(()) => "forwarded".into(),
                            Err(e) => e,
                        };
                        self.picked = None;
                        None
                    }
                    KeyCode::Esc => None,
                    _ => Some(Popup::Forward { at, to }),
                }
            }
            Popup::InviteToGroup(at) => {
                let Some(g) = self.group_index() else { return };
                let rows = self.engine.invitable(g);
                match k.code {
                    KeyCode::Up => Some(Popup::InviteToGroup(at.saturating_sub(1))),
                    KeyCode::Down => Some(Popup::InviteToGroup(
                        (at + 1).min(rows.len().saturating_sub(1)),
                    )),
                    KeyCode::Enter => {
                        if let Some(&i) = rows.get(at)
                            && let Err(e) = self.engine.invite_to_group(g, i)
                        {
                            self.status = e;
                        } else if !rows.is_empty() {
                            self.status = "invited: they join from their chat with you".into();
                        }
                        None
                    }
                    KeyCode::Esc => None,
                    _ => Some(Popup::InviteToGroup(at)),
                }
            }
            Popup::RemoveMember(who) => {
                if k.code == KeyCode::Char('y')
                    && let Some(g) = self.group_index()
                    && let Err(e) = self.engine.remove_from_group(g, &who)
                {
                    self.status = e;
                }
                None
            }
            Popup::LinkCode(code) => match k.code {
                KeyCode::Char('c') => {
                    copy_to_clipboard(&code);
                    self.status = "link code copied (if your terminal supports OSC 52)".into();
                    Some(Popup::LinkCode(code))
                }
                _ => None,
            },
            Popup::Devices(at) => {
                let rows = self.engine.device_rows();
                let main = self.engine.is_primary();
                match k.code {
                    KeyCode::Up => Some(Popup::Devices(at.saturating_sub(1))),
                    KeyCode::Down => {
                        Some(Popup::Devices((at + 1).min(rows.len().saturating_sub(1))))
                    }
                    KeyCode::Char('a') if main => Some(Popup::AddDevice(Input::default())),
                    KeyCode::Char('r') if main => rows
                        .get(at)
                        .map(|(k, l)| Popup::RenameDevice(*k, Input::from(l.as_str()))),
                    KeyCode::Char('x') if main => rows
                        .get(at)
                        .map(|(k, _)| *k)
                        .filter(|k| *k != self.engine.me())
                        .map(Popup::RemoveDevice),
                    KeyCode::Esc => None,
                    _ => Some(Popup::Devices(at)),
                }
            }
            Popup::AddDevice(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => match self.engine.add_device(&s) {
                    Ok(i) => Some(Popup::Link(i)),
                    Err(e) => {
                        self.status = e;
                        None
                    }
                },
                false => Some(Popup::AddDevice(s)),
            },
            Popup::RenameDevice(key, mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    if let Err(e) = self.engine.set_device_label(&key, &s) {
                        self.status = e;
                    }
                    Some(Popup::Devices(0))
                }
                false => Some(Popup::RenameDevice(key, s)),
            },
            Popup::Link(i) => match k.code {
                KeyCode::Char('y') if self.engine.link_waiting(i) => Some(Popup::Link(i)),
                KeyCode::Char('y') => {
                    self.engine.confirm_link(i);
                    self.status = LINK_CONFIRMED.into();
                    None
                }
                KeyCode::Char('n') => {
                    self.engine.delete(i);
                    self.status = "linking cancelled".into();
                    None
                }
                _ => Some(Popup::Link(i)),
            },
            Popup::RemoveDevice(key) => {
                if k.code == KeyCode::Char('y') {
                    match self.engine.remove_device(&key) {
                        Ok(()) => self.status = "device removed".into(),
                        Err(e) => self.status = e,
                    }
                }
                None
            }
            Popup::LeaveGroup => {
                if k.code == KeyCode::Char('y')
                    && let Some(g) = self.group_index()
                    && let Err(e) = self.engine.leave_group(g)
                {
                    self.status = e;
                }
                None
            }
            Popup::Rename(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    let name = s.trim();
                    let name = (!name.is_empty()).then(|| name.chars().take(64).collect());
                    self.engine.edit(i, |c| c.petname = name);
                    None
                }
                false => Some(Popup::Rename(s)),
            },
            Popup::Add(_)
            | Popup::AddDevice(_)
            | Popup::RenameDevice(..)
            | Popup::NewGroup(_)
            | Popup::Rename(_)
            | Popup::Incognito(_)
            | Popup::Search(_)
            | Popup::Bridges { .. } => None,
        };
        self.popup = keep;
    }

    /// Settings forms. `Ok` carries what to tell the user.
    fn submit(&mut self, f: &Form) -> Result<String, String> {
        match f.kind {
            Kind::Backup => {
                if f.fields[0].trim().is_empty() {
                    return Err("where should it go? A USB stick, for example".into());
                }
                let path = form::expand(&f.fields[0]);
                self.engine.backup(&path, f.new_password(1)?)?;
                Ok(format!(
                    "Backup written to {}.\nMove it off this device: a wipe erases it only while it\nstays at this path. Restore: Ctrl+B on the first-run screen.",
                    path.display()
                ))
            }
            Kind::Duress => {
                let duress = match f.fields[1].is_empty() {
                    true => None,
                    false => Some(f.new_password(1)?),
                };
                self.engine.set_duress(&f.fields[0], duress)
            }
            Kind::Password => {
                self.engine
                    .change_password(&f.fields[0], f.new_password(1)?)?;
                Ok("Password changed.".into())
            }
            Kind::Export => {
                let export = match self.group_index() {
                    Some(g) => self.engine.export_group(g),
                    None => self.current().and(self.engine.export_chat(self.selected)),
                };
                let (name, text) = export.ok_or("open a chat first")?;
                let dir = form::expand(&f.fields[0]);
                let path = crate::data::save_new(&dir, &name, text.as_bytes())
                    .map_err(|e| format!("cannot save: {e}"))?;
                Ok(format!(
                    "Exported to {}. It is not encrypted: delete it when done.",
                    path.display()
                ))
            }
            Kind::SendFile => {
                let (name, bytes) = data::read_document(&form::expand(&f.fields[0]))?;
                match self.group_index() {
                    Some(g) => self.engine.send_group_file(g, &name, bytes)?,
                    None => self.engine.send_file(self.selected, &name, bytes)?,
                }
                Ok(format!("Sending {name}; progress shows in the chat."))
            }
            Kind::SaveFile if self.group_index().is_some() => {
                let g = self.group_index().ok_or("no group open")?;
                let m = self
                    .engine
                    .unsaved_group_file(g)
                    .ok_or("no unsaved file in this group")?;
                if f.fields[0].trim() == "DISCARD" {
                    self.engine.discard_group_file(g, m);
                    return Ok("File discarded.".into());
                }
                let path = self
                    .engine
                    .save_group_file(g, m, &form::expand(&f.fields[0]))?;
                Ok(format!("Saved to {}.", path.display()))
            }
            Kind::SaveFile => {
                let i = self.selected;
                let m = self
                    .engine
                    .unsaved_file(i)
                    .ok_or("no unsaved file in this chat")?;
                if f.fields[0].trim() == "DISCARD" {
                    self.engine.discard_file(i, m);
                    return Ok("File discarded.".into());
                }
                let path = self.engine.save_file(i, m, &form::expand(&f.fields[0]))?;
                Ok(format!("Saved to {}.", path.display()))
            }
            Kind::Setup | Kind::Restore | Kind::Wipe => Ok(String::new()),
        }
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

    /// Ctrl+L on the first-run screen sets up a device to link: no nickname
    /// asked, and the link code comes once the relay is reached (ADR-028).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn ctrl_l_at_first_run_sets_up_a_device_to_link() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("termchat-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        // A folder others can read would put a warning in front of the code.
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let mut app = App::new(dir.join("state"));
        let Screen::Setup(f) = &mut app.screen else {
            panic!("not the first-run screen");
        };
        f.fields[0] = "ws://127.0.0.1:9/".into();
        f.fields[1] = "".into();
        f.fields[2] = "n".into();
        f.fields[3] = "".into();
        f.fields[4] = "correct horse battery staple violin".into();
        f.fields[5] = "correct horse battery staple violin".into();
        app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        let Screen::Main(m) = &app.screen else {
            let Screen::Setup(f) = &app.screen else {
                panic!()
            };
            panic!("still on setup: {}", f.error);
        };
        assert_eq!(m.engine.data.nickname, NEW_DEVICE_NICKNAME);
        assert!(m.linking, "waits for the relay");
        assert!(m.popup.is_none(), "nothing in the way of the code");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each chat keeps what was typed in it, across switching and a restart.
    #[tokio::test(flavor = "multi_thread")]
    async fn drafts_stay_with_their_chat() {
        let dir = std::env::temp_dir().join(format!("termchat-drafts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let password = "correct horse battery staple violin";
        let setup = termchat_engine::Setup {
            relay_url: "ws://127.0.0.1:9/",
            onion_url: None,
            tor: false,
            nickname: "alice",
            password,
        };
        let Ok(engine) = Engine::setup(&dir.join("state"), &setup) else {
            panic!("setup");
        };
        let mut main = Main::new(engine, &dir);
        assert!(main.engine.create_group("one").is_ok());
        assert!(main.engine.create_group("two").is_ok());
        let first = main.engine.data.people();
        main.select(first);
        main.input.insert("half a thought");
        main.select(first + 1);
        assert!(main.input.is_empty(), "the other chat has its own field");
        main.input.insert("for two");
        main.select(first);
        assert_eq!(&*main.input, "half a thought");
        main.persist();
        drop(main);
        let Ok(Unlocked::Open(engine)) = Engine::unlock(&dir.join("state"), password.as_bytes())
        else {
            panic!("unlock");
        };
        assert_eq!(engine.data.groups[0].draft, "half a thought");
        assert_eq!(engine.data.groups[1].draft, "for two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// In an open group, Ctrl+L and Ctrl+G are documents, not "not in a
    /// group" (ADR-027 amendment; a bug caught before 0.15.0 went out).
    #[tokio::test(flavor = "multi_thread")]
    async fn ctrl_l_in_a_group_opens_the_file_form() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let dir = std::env::temp_dir().join(format!("termchat-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let setup = termchat_engine::Setup {
            relay_url: "ws://127.0.0.1:9/",
            onion_url: None,
            tor: false,
            nickname: "alice",
            password: "correct horse battery staple violin",
        };
        let Ok(engine) = Engine::setup(&dir.join("state"), &setup) else {
            panic!("setup");
        };
        let mut main = Main::new(engine, &dir);
        assert!(main.engine.create_group("docs").is_ok());
        main.selected = main.engine.data.people();
        let mut settings = Settings::default();
        let ctrl_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);
        assert_eq!(main.group_index(), Some(0), "the group is open");
        // Whatever setup showed (the backup reminder) is dismissed first.
        main.popup = None;
        main.on_key(ctrl_l, true, &mut settings);
        assert!(
            matches!(&main.popup, Some(Popup::Form(f)) if f.kind == Kind::SendFile),
            "{}",
            main.status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timer_step_names_the_next_press() {
        assert_eq!(timer_step(None), "off → 5m");
        assert_eq!(timer_step(Some(3600)), "1h → 1d");
        assert_eq!(timer_step(Some(604_800)), "7d → off");
    }
}
