//! Drawing. Every string that came from another person goes through `clean`.
//!
//! Look after herdr: the main area keeps the terminal's background, the theme
//! paints bars and windows, and whatever is under a window is dimmed.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use termchat_core::{fingerprint, safety_number};

use crate::app::{App, Main, Popup, Screen, Unlock, now};
use crate::data::{Contact, Status};
use crate::form::Form;
use crate::settings::Settings;
use crate::theme::Palette;

/// The chat needs at least this much room next to the contact list, or the
/// list is hidden; Up/Down still switch chats.
const CHAT_MIN: u16 = 36;
/// Rows the message box grows to before it scrolls.
const INPUT_ROWS: usize = 5;

const HELP: &str = "\
Enter        send message
Up / Down    choose contact
Ctrl+A       add contact (paste their invite)
Ctrl+O       show your invite
Ctrl+F       safety number: verify the contact
Ctrl+Y       accept contact request
Ctrl+D       delete contact
Ctrl+B       block / unblock contact
Ctrl+X       report contact to the relay operator (and block)
Ctrl+R       rename contact (only on this device)
Ctrl+T       disappearing messages: off/5m/1h/1d/1w
Ctrl+E       no history: keep this chat in memory only
Ctrl+P       menu: backup, password, duress, Tor, theme, mouse, wipe
Ctrl+K       lock now (also after 5 min idle)
Ctrl+Q       quit
? / F1       this help

PgUp / PgDn  scroll the chat (or the mouse wheel)
Alt+← / →    word left / right   Home / End   start / end
Ctrl+W       delete word         Ctrl+U       delete to start
Mouse        click a contact, right-click for its actions;
             drag the list edge to resize; Shift+drag selects text

  ● unread   ? request   ! not verified   × blocked
  … sending   ✓ on relay   ✗ failed

Need help now? You are not alone.
  Helplines in every country: https://findahelpline.com
  Emergency: 112 (EU), 911 (US)   Suicide & Crisis Lifeline (US): 988
  Samaritans (UK, IE): 116 123
  Human trafficking hotline (US): 1-888-373-7888";

/// Drops control and bidi-override characters: a peer must not be able to
/// move the cursor, clear the screen or reorder text in our terminal.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|&c| {
            !c.is_control()
                && !matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

/// What the contact is called here. Names are chosen by the peer, so an
/// unverified contact, or one whose name collides with another contact or with
/// ours, also shows the start of its fingerprint: "Alice·48213" (ADR-007).
pub fn name(m: &Main, c: &Contact) -> String {
    let base = |c: &Contact| {
        let nick = m.core().nickname(&c.key).map(str::to_owned);
        clean(&c.petname.clone().or(nick).unwrap_or_default())
    };
    let fp = fingerprint(&c.key);
    let name = base(c);
    if name.is_empty() {
        return fp[..5].to_string();
    }
    // O(contacts) per name, O(n²) per frame; cache if lists grow large.
    let collides = name == clean(&m.data.nickname)
        || m.data
            .contacts
            .iter()
            .any(|o| o.key != c.key && base(o) == name);
    match !c.verified || collides {
        true => format!("{name}·{}", &fp[..5]),
        false => name,
    }
}

fn age(at: u64) -> String {
    match now().saturating_sub(at) {
        0..60 => "now".into(),
        s => span(s),
    }
}

fn span(s: u64) -> String {
    match s {
        0..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// Cuts `s` to `w` columns, with "…" if anything was cut.
fn fit(s: &str, w: usize) -> String {
    if Span::raw(s).width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if Span::raw(format!("{out}{c}…")).width() > w {
            break;
        }
        out.push(c);
    }
    out + "…"
}

/// Text on an accent background; the `terminal` theme has no panel colour.
fn on_accent(t: &Palette) -> Color {
    match t.panel_bg {
        Color::Reset => Color::Black,
        c => c,
    }
}

pub fn render(f: &mut Frame, app: &App) {
    let t = app.settings.palette();
    let mut h = app.hits.borrow_mut();
    *h = Hits::default();
    match &app.screen {
        Screen::Setup(s) => form(f, t, &mut h, s),
        Screen::Unlock(u) => unlock(f, t, &mut h, u),
        Screen::Main(m) => main(f, t, &mut h, m, &app.settings),
    }
}

/// Where things were drawn in the last frame, for mouse clicks.
#[derive(Debug, Clone, Default)]
pub struct Hits {
    /// Contact rows, and the index of the first one shown.
    pub list: Rect,
    pub list_offset: usize,
    pub add: Rect,
    pub menu: Rect,
    /// The column between the list and the chat, for resizing.
    pub edge: Option<u16>,
    pub history: Rect,
    /// How far the history can scroll up.
    pub max_scroll: usize,
    /// The open window, if any: clicks outside close it.
    pub window: Option<Rect>,
    /// Buttons and menu rows: a click is the key they stand for.
    pub keys: Vec<(Rect, KeyEvent)>,
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// A button: label, whether it is the primary action, and its key.
type Button<'a> = (&'a str, bool, KeyEvent);

/// A centred window on the panel colour: body, then a row of buttons. The
/// first `rows.len()` body lines are menu rows, clickable as those keys.
#[allow(clippy::too_many_arguments)]
fn window(
    f: &mut Frame,
    t: &Palette,
    h: &mut Hits,
    title: &str,
    mut body: Vec<Line<'_>>,
    rows: &[KeyEvent],
    buttons: &[Button],
    w: u16,
    danger: bool,
) {
    let edge = if danger { t.red } else { t.accent };
    let mut spots = Vec::new();
    if !buttons.is_empty() {
        let mut row = Vec::new();
        let mut x = 0;
        for (label, primary, k) in buttons {
            let style = match primary {
                true => Style::new()
                    .bg(edge)
                    .fg(on_accent(t))
                    .add_modifier(Modifier::BOLD),
                false => Style::new().bg(t.surface0).fg(t.text),
            };
            let label = Span::styled(format!(" {label} "), style);
            let width = u16::try_from(label.width()).unwrap_or(u16::MAX);
            spots.push((x, width, *k));
            x += width + 2;
            row.push(label);
            row.push(Span::raw("  "));
        }
        body.push(Line::from(""));
        body.push(Line::from(row));
    }
    let w = w.min(f.area().width);
    let text = Paragraph::new(body).wrap(Wrap { trim: false });
    let lines = u16::try_from(text.line_count(w.saturating_sub(4))).unwrap_or(u16::MAX);
    let area = centered(f.area(), w, lines.saturating_add(2));
    f.render_widget(Clear, area);
    f.render_widget(
        text.style(Style::new().bg(t.panel_bg).fg(t.text)).block(
            Block::bordered()
                .border_style(Style::new().fg(edge))
                .title(Span::styled(
                    format!(" {title} "),
                    Style::new().fg(t.text).add_modifier(Modifier::BOLD),
                ))
                .padding(Padding::horizontal(1)),
        ),
        area,
    );
    h.window = Some(area);
    for (i, k) in (0u16..).zip(rows) {
        let r = Rect::new(area.x + 1, area.y + 1 + i, area.width.saturating_sub(2), 1);
        h.keys.push((r.intersection(area), *k));
    }
    let y = area.y + lines;
    for (x, width, k) in spots {
        h.keys
            .push((Rect::new(area.x + 2 + x, y, width, 1).intersection(area), k));
    }
}

/// An input field `w` columns wide, scrolled so the cursor (a char index)
/// stays visible; the cursor is a reversed cell.
fn field(t: &Palette, value: &str, cursor: Option<usize>, w: usize) -> Line<'static> {
    let chars: Vec<char> = value.chars().collect();
    let at = cursor.unwrap_or(0);
    let start = (at + 1).saturating_sub(w);
    let shown: String = chars.iter().skip(start).take(w).collect();
    let style = Style::new().bg(t.surface0).fg(t.text);
    let mut spans = Vec::new();
    match cursor {
        Some(at) => {
            let mut it = shown.chars();
            let before: String = it.by_ref().take(at - start).collect();
            let under = it.next().map_or(" ".into(), String::from);
            spans.push(Span::styled(before, style));
            spans.push(Span::styled(under, style.add_modifier(Modifier::REVERSED)));
            spans.push(Span::styled(it.collect::<String>(), style));
        }
        None => spans.push(Span::styled(shown, style)),
    }
    let pad = w.saturating_sub(Line::from(spans.clone()).width());
    spans.push(Span::styled(" ".repeat(pad), style));
    Line::from(spans)
}

/// A field showing bullets instead of the secret.
fn secret_field(t: &Palette, value: &str, cursor: Option<usize>, w: usize) -> Line<'static> {
    field(t, &"•".repeat(value.chars().count()), cursor, w)
}

/// Lines like "Ctrl+A   add contact": the key part in the accent colour.
fn keyed<'a>(t: &Palette, text: &'a str) -> Vec<Line<'a>> {
    text.lines()
        .map(|l| match l.split_once("  ") {
            Some((key, rest)) if !key.is_empty() && !key.starts_with(' ') => Line::from(vec![
                Span::styled(key, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::raw(format!("  {rest}")),
            ]),
            _ => Line::from(l),
        })
        .collect()
}

/// Menu rows "k  label", the highlighted one on the selection colour.
fn menu_rows(t: &Palette, items: &[(char, String)], at: usize, w: usize) -> Vec<Line<'static>> {
    items
        .iter()
        .enumerate()
        .map(|(i, (k, label))| {
            let row = match i == at {
                true => Style::new().bg(t.selection_bg),
                false => Style::new(),
            };
            let text = format!("  {label}");
            let pad = w.saturating_sub(1 + Span::raw(&text).width());
            Line::from(vec![
                Span::styled(k.to_string(), row.fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{text}{}", " ".repeat(pad)), row.fg(t.text)),
            ])
        })
        .collect()
}

/// Any form, centred, over whatever is below (setup screen or a settings popup).
fn form(f: &mut Frame, t: &Palette, h: &mut Hits, form: &Form) {
    const W: u16 = 76;
    let mut lines: Vec<Line> = form
        .kind
        .info()
        .lines()
        .map(|l| Line::styled(l, Style::new().fg(t.subtext0)))
        .collect();
    for (i, ((label, secret), value)) in form.kind.labels().iter().zip(&form.fields).enumerate() {
        let focused = i == form.field;
        let style = match focused {
            true => Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
            false => Style::new().fg(t.subtext0),
        };
        let cursor = focused.then(|| value.cursor_chars());
        let w = usize::from(W) - 4;
        lines.push(Line::from(""));
        lines.push(Line::styled(*label, style));
        lines.push(match secret {
            true => secret_field(t, value, cursor, w),
            false => field(t, value, cursor, w),
        });
    }
    if let Some(phrase) = &form.generated {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("Generated: {}  (write it down)", phrase.as_str()),
            Style::new().fg(t.green),
        ));
    }
    if !form.error.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(form.error.as_str(), Style::new().fg(t.red)));
    }
    let danger = form.kind == crate::form::Kind::Wipe;
    let leave = match form.kind {
        crate::form::Kind::Setup => ("^q quit", false, ctrl('q')),
        _ => ("esc cancel", false, key(KeyCode::Esc)),
    };
    let buttons = [
        ("↵ continue", true, key(KeyCode::Enter)),
        ("tab next field", false, key(KeyCode::Tab)),
        leave,
    ];
    window(
        f,
        t,
        h,
        form.kind.title().trim(),
        lines,
        &[],
        &buttons,
        W,
        danger,
    );
}

fn unlock(f: &mut Frame, t: &Palette, h: &mut Hits, u: &Unlock) {
    let mut lines = vec![
        Line::styled(
            "Password",
            Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        secret_field(t, &u.input, Some(u.input.cursor_chars()), 46),
    ];
    if !u.error.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(u.error.as_str(), Style::new().fg(t.red)));
    }
    let buttons = [
        ("↵ unlock", true, key(KeyCode::Enter)),
        ("^q quit", false, ctrl('q')),
    ];
    window(
        f,
        t,
        h,
        "termchat · locked",
        lines,
        &[],
        &buttons,
        50,
        false,
    );
}

fn main(f: &mut Frame, t: &Palette, h: &mut Hits, m: &Main, settings: &Settings) {
    let area = f.area();
    let [body, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let chat = match area.width >= settings.sidebar + CHAT_MIN {
        true => {
            let [list, chat] =
                Layout::horizontal([Constraint::Length(settings.sidebar), Constraint::Min(1)])
                    .areas(body);
            sidebar(f, t, h, m, list);
            chat
        }
        false => body,
    };
    chat_pane(f, t, h, m, chat);
    status_bar(f, t, m, bar);

    if let Some(popup) = &m.popup {
        f.buffer_mut()
            .set_style(area, Style::new().add_modifier(Modifier::DIM));
        popup_pane(f, t, h, m, popup, settings);
    }
}

fn sidebar(f: &mut Frame, t: &Palette, h: &mut Hits, m: &Main, area: Rect) {
    let edge = Block::new()
        .borders(Borders::RIGHT)
        .border_style(Style::new().fg(t.surface_dim));
    let inner = edge.inner(area);
    f.render_widget(edge, area);
    h.edge = Some(area.right().saturating_sub(1));
    let [head, list, foot] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let muted = Style::new().fg(t.overlay0);
    f.render_widget(
        Paragraph::new(Line::styled(
            " contacts",
            muted.add_modifier(Modifier::BOLD),
        )),
        head,
    );

    let width = usize::from(list.width);
    let items: Vec<ListItem> = m
        .data
        .contacts
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let request = m.core().is_request(&c.key) == Some(true);
            let (mark, colour) = match () {
                _ if c.blocked => ("×", t.red),
                _ if request => ("?", t.yellow),
                _ if c.unread > 0 => ("●", t.accent),
                _ if !c.verified => ("!", t.peach),
                _ => (" ", t.overlay0),
            };
            let badge = match c.unread {
                0 => String::new(),
                1..=99 => c.unread.to_string(),
                _ => "99+".into(),
            };
            let label = fit(&name(m, c), width.saturating_sub(5 + badge.len()));
            let pad = width.saturating_sub(4 + Span::raw(&label).width() + badge.len());
            let style = match i == m.selected {
                true => Style::new().fg(t.text).add_modifier(Modifier::BOLD),
                false => Style::new().fg(t.subtext0),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {mark} "), Style::new().fg(colour)),
                Span::styled(label, style),
                Span::raw(" ".repeat(pad)),
                Span::styled(
                    badge,
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
            ]))
        })
        .collect();
    if items.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(" no contacts yet", muted)),
            list,
        );
    } else {
        let mut state = ListState::default().with_selected(Some(m.selected));
        f.render_stateful_widget(
            List::new(items).highlight_style(Style::new().bg(t.active_row_bg)),
            list,
            &mut state,
        );
        h.list = list;
        h.list_offset = state.offset();
    }

    h.add = Rect::new(foot.x, foot.y, 6, 1).intersection(foot);
    h.menu = Rect::new(foot.right().saturating_sub(7), foot.y, 7, 1).intersection(foot);
    f.render_widget(Paragraph::new(Line::styled(" + add", muted)), foot);
    f.render_widget(
        Paragraph::new(Line::styled("≡ menu ", muted)).alignment(Alignment::Right),
        foot,
    );
}

/// Breaks `s` into rows of at most `w` columns, by character, and finds the
/// row and column of the cursor (a char index).
fn wrap(s: &str, w: usize, cursor: usize) -> (Vec<String>, (usize, usize)) {
    let w = w.max(2);
    let mut rows = vec![String::new()];
    let mut col = 0;
    let mut at = None;
    for (i, c) in s.chars().enumerate() {
        let cw = Span::raw(c.to_string()).width();
        if col + cw > w {
            rows.push(String::new());
            col = 0;
        }
        if i == cursor {
            at = Some((rows.len() - 1, col));
        }
        if let Some(r) = rows.last_mut() {
            r.push(c);
        }
        col += cw;
    }
    if at.is_none() && col >= w {
        rows.push(String::new());
        col = 0;
    }
    let at = at.unwrap_or((rows.len() - 1, col));
    (rows, at)
}

fn chat_pane(f: &mut Frame, t: &Palette, h: &mut Hits, m: &Main, area: Rect) {
    let typed = wrap(
        &m.input,
        usize::from(area.width.saturating_sub(4)),
        m.input.cursor_chars(),
    );
    let shown = typed.0.len().min(INPUT_ROWS);
    let [head, history, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(u16::try_from(shown).unwrap_or(1) + 2),
    ])
    .areas(area);
    let bar = Style::new().bg(t.panel_bg).fg(t.text);
    let bold = Style::new().fg(t.text).add_modifier(Modifier::BOLD);

    let Some(c) = m.current() else {
        f.render_widget(
            Paragraph::new(Line::styled(" termchat", bold)).style(bar),
            head,
        );
        let key = Style::new().fg(t.accent).add_modifier(Modifier::BOLD);
        let lines = vec![
            Line::styled("No contacts yet.", bold),
            Line::from(""),
            Line::from(vec![
                Span::styled("Ctrl+O", key),
                Span::raw("  show your invite, send it to a friend"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+A", key),
                Span::raw("  add a friend from their invite"),
            ]),
        ];
        let [_, mid, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(4),
            Constraint::Fill(1),
        ])
        .areas(history);
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), mid);
        return input_bar(f, t, m, input, typed);
    };

    let them = name(m, c);
    let mut title = vec![Span::styled(format!(" {them}"), bold)];
    let mut flag = |text: String, colour: Color| {
        title.push(Span::styled(format!("   {text}"), Style::new().fg(colour)));
    };
    if !c.verified {
        flag("! not verified (Ctrl+F)".into(), t.peach);
    }
    if let Some(s) = c.expires_in {
        flag(format!("⏱ {}", span(u64::from(s))), t.teal);
    }
    if c.no_history {
        flag("no history".into(), t.mauve);
    }
    let waiting = c.hidden.len() + c.held_back as usize;
    if waiting > 0 {
        flag(
            format!("{waiting} hidden until you accept (Ctrl+Y)"),
            t.yellow,
        );
    }
    f.render_widget(Paragraph::new(Line::from(title)).style(bar), head);

    // Messages in groups, like most chat apps: name and age over a run of
    // messages from one side, so wrapped text stays flush left.
    let muted = Style::new().fg(t.overlay0);
    let mut lines: Vec<Line> = Vec::new();
    let mut prev: Option<&crate::data::Msg> = None;
    for msg in &c.messages {
        let text = clean(&msg.text);
        if msg.status == Status::System {
            lines.push(Line::styled(text, muted.add_modifier(Modifier::ITALIC)));
            prev = None;
            continue;
        }
        let same = prev.is_some_and(|p| p.mine == msg.mine && msg.at.saturating_sub(p.at) < 300);
        if !same {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            let (who, colour) = match msg.mine {
                true => ("you".to_string(), t.accent),
                false => (them.clone(), t.mauve),
            };
            lines.push(Line::from(vec![
                Span::styled(who, Style::new().fg(colour).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", age(msg.at)), muted),
            ]));
        }
        prev = Some(msg);
        let mut spans = vec![Span::styled(text, Style::new().fg(t.text))];
        match &msg.status {
            Status::Queued | Status::Sending(_) => spans.push(Span::styled(" …", muted)),
            Status::Sent => spans.push(Span::styled(" ✓", muted)),
            Status::Failed(why) => {
                spans.push(Span::styled(format!(" ✗ {why}"), Style::new().fg(t.red)))
            }
            _ => {}
        }
        lines.push(Line::from(spans));
    }

    // Bottom-anchored: `scroll` counts rows up from the newest one.
    let pane = history.inner(Margin::new(1, 1));
    let text = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = text.line_count(pane.width);
    let max = total.saturating_sub(usize::from(pane.height));
    let scroll = m.scroll.min(max);
    let top = max - scroll;
    f.render_widget(
        text.scroll((u16::try_from(top).unwrap_or(u16::MAX), 0)),
        pane,
    );
    h.history = pane;
    h.max_scroll = max;
    if max > 0 {
        let mut state = ScrollbarState::new(max).position(top);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("▕"))
                .track_style(Style::new().fg(t.surface_dim))
                .thumb_symbol("▐")
                .thumb_style(Style::new().fg(t.overlay0)),
            history.inner(Margin::new(0, 1)),
            &mut state,
        );
    }
    if scroll > 0 {
        let hint = Span::styled(" ↓ PgDn ", Style::new().bg(t.panel_bg).fg(t.accent));
        let row = Rect::new(pane.x, pane.bottom().saturating_sub(1), pane.width, 1);
        f.render_widget(Paragraph::new(hint).alignment(Alignment::Right), row);
    }
    input_bar(f, t, m, input, typed);
}

fn input_bar(
    f: &mut Frame,
    t: &Palette,
    m: &Main,
    area: Rect,
    (rows, (row, col)): (Vec<String>, (usize, usize)),
) {
    let style = Style::new().bg(t.surface0).fg(t.text);
    f.render_widget(Block::new().style(style), area);
    let text = area.inner(Margin::new(1, 1));
    let prompt = Span::styled("› ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    f.render_widget(Paragraph::new(prompt), text);
    let text = Rect::new(
        text.x + 2,
        text.y,
        text.width.saturating_sub(2),
        text.height,
    );
    if m.input.is_empty() {
        let hint = match m.current() {
            Some(_) => "message",
            None => "add a contact first (Ctrl+A)",
        };
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::new().fg(t.overlay0))),
            text,
        );
    }
    let first = (row + 1).saturating_sub(INPUT_ROWS);
    let lines: Vec<Line> = rows.into_iter().skip(first).map(Line::from).collect();
    f.render_widget(Paragraph::new(lines), text);
    if m.popup.is_none() {
        let x = text.x + u16::try_from(col).unwrap_or(0);
        let y = text.y + u16::try_from(row - first).unwrap_or(0);
        f.set_cursor_position((x.min(text.right()), y));
    }
}

fn status_bar(f: &mut Frame, t: &Palette, m: &Main, area: Rect) {
    let link = match (m.connected, m.data.tor) {
        (true, true) => (" ● online via Tor", t.green),
        (false, true) => (" ○ connecting via Tor…", t.yellow),
        (true, false) => (" ● online", t.green),
        (false, false) => (" ○ connecting…", t.yellow),
    };
    let mut left = vec![
        Span::styled(link.0, Style::new().fg(link.1)),
        Span::styled(
            format!("  {}", clean(&m.data.nickname)),
            Style::new().fg(t.subtext0),
        ),
    ];
    if !m.data.backed_up {
        left.push(Span::styled(
            "  ! no backup: Ctrl+P",
            Style::new().fg(t.peach),
        ));
    }
    if !m.status.is_empty() {
        left.push(Span::styled(
            format!("  {}", m.status),
            Style::new().fg(t.yellow),
        ));
    }
    let key = Style::new().fg(t.accent).add_modifier(Modifier::BOLD);
    let word = Style::new().fg(t.overlay0);
    let mut right = Vec::new();
    for (k, w) in [
        ("^P", "menu"),
        ("?", "keys"),
        ("^K", "lock"),
        ("^Q", "quit"),
    ] {
        right.push(Span::styled(k, key));
        right.push(Span::styled(format!(" {w}  "), word));
    }
    let (left, right) = (Line::from(left), Line::from(right));
    f.render_widget(Block::new().style(Style::new().bg(t.panel_bg)), area);
    if left.width() + right.width() <= usize::from(area.width) {
        f.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
    }
    f.render_widget(Paragraph::new(left), area);
}

fn popup_pane(
    f: &mut Frame,
    t: &Palette,
    h: &mut Hits,
    m: &Main,
    popup: &Popup,
    settings: &Settings,
) {
    let plain = |s: &str| -> Vec<Line> { s.lines().map(|l| Line::from(l.to_string())).collect() };
    let strong = Style::new().fg(t.accent).add_modifier(Modifier::BOLD);
    let esc = key(KeyCode::Esc);
    let close: &[Button] = &[("esc close", false, esc)];
    let cancel = ("esc cancel", false, esc);
    match popup {
        Popup::Form(fm) => form(f, t, h, fm),
        Popup::Notice(text) => {
            let ok = [("↵ ok", true, key(KeyCode::Enter))];
            window(f, t, h, "note", plain(text), &[], &ok, 70, false);
        }
        Popup::Menu(at) | Popup::Actions(at) => {
            let (title, items) = match popup {
                Popup::Menu(_) => ("menu".to_string(), m.menu(settings)),
                _ => (
                    m.current().map(|c| name(m, c)).unwrap_or_default(),
                    m.actions(),
                ),
            };
            let keys: Vec<KeyEvent> = items.iter().map(|(c, _)| key(KeyCode::Char(*c))).collect();
            let lines = menu_rows(t, &items, *at, 44);
            window(f, t, h, &title, lines, &keys, close, 48, false);
        }
        Popup::Help => window(f, t, h, "keys", keyed(t, HELP), &[], close, 72, false),
        Popup::Add(s) => {
            let lines = vec![
                Line::from("Paste their invite and press Enter."),
                Line::from(""),
                field(t, s, Some(s.cursor_chars()), 66),
            ];
            let buttons = [("↵ add", true, key(KeyCode::Enter)), cancel];
            window(f, t, h, "add contact", lines, &[], &buttons, 70, false);
        }
        Popup::Invite(invite) => {
            let lines = vec![
                Line::from("Send this to a friend. It holds only public keys."),
                Line::from(""),
                Line::styled(invite.as_str(), strong),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Your fingerprint  ", Style::new().fg(t.subtext0)),
                    Span::raw(fingerprint(&m.me())),
                ]),
            ];
            let buttons = [
                ("c copy to clipboard", true, key(KeyCode::Char('c'))),
                ("esc close", false, esc),
            ];
            window(f, t, h, "your invite", lines, &[], &buttons, 70, false);
        }
        Popup::Verify => {
            let Some(c) = m.current() else { return };
            let number = safety_number(&m.me(), &c.key);
            let groups: Vec<&str> = number.split(' ').collect();
            let mut lines = vec![
                Line::from(format!(
                    "Compare with {} in person or by phone.",
                    name(m, c)
                )),
                Line::from("Same numbers on both screens = nobody is in the middle."),
                Line::from(""),
            ];
            lines.extend(groups.chunks(4).map(|g| Line::styled(g.join(" "), strong)));
            lines.push(Line::from(""));
            lines.push(match c.verified {
                true => Line::styled("VERIFIED", Style::new().fg(t.green)),
                false => Line::styled("not verified", Style::new().fg(t.peach)),
            });
            let buttons = [
                ("v toggle verified", true, key(KeyCode::Char('v'))),
                ("esc close", false, esc),
            ];
            window(f, t, h, "safety number", lines, &[], &buttons, 64, false);
        }
        Popup::Rename(s) => {
            let lines = vec![
                Line::from("New name, only on this device. Empty resets it."),
                Line::from(""),
                field(t, s, Some(s.cursor_chars()), 46),
            ];
            let buttons = [("↵ save", true, key(KeyCode::Enter)), cancel];
            window(f, t, h, "rename", lines, &[], &buttons, 50, false);
        }
        Popup::Report(text) => {
            let mut lines = plain(
                "The relay operator will READ the text below. Reports from 3 different\npeople this key wrote to ban it on this relay. The contact gets blocked.\n",
            );
            lines.extend(
                clean(text)
                    .lines()
                    .map(|l| Line::styled(l.to_string(), Style::new().fg(t.subtext0))),
            );
            let buttons = [
                ("y report and block", true, key(KeyCode::Char('y'))),
                cancel,
            ];
            window(
                f,
                t,
                h,
                "report to the relay operator",
                lines,
                &[],
                &buttons,
                76,
                true,
            );
        }
        Popup::Delete => {
            let lines = plain("Delete this contact and the chat?");
            let buttons = [
                ("y delete", true, key(KeyCode::Char('y'))),
                ("esc keep", false, esc),
            ];
            window(f, t, h, "delete contact", lines, &[], &buttons, 50, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_escapes_and_bidi() {
        assert_eq!(clean("hi\x1b[2Jthere\u{202E}lol\nok"), "hi[2Jtherelol ok");
        assert_eq!(clean("Ёжик 🦔"), "Ёжик 🦔");
    }

    #[test]
    fn wrap_places_the_cursor() {
        let (rows, at) = wrap("abcdef", 4, 5);
        assert_eq!(rows, ["abcd", "ef"]);
        assert_eq!(at, (1, 1));
        // Cursor at the end of a full row moves to a new one.
        let (rows, at) = wrap("abcd", 4, 4);
        assert_eq!(rows, ["abcd", ""]);
        assert_eq!(at, (1, 0));
        assert_eq!(wrap("", 4, 0), (vec![String::new()], (0, 0)));
        // Wide characters take two columns.
        assert_eq!(wrap("日本語", 4, 3).0, ["日本", "語"]);
    }

    #[test]
    fn fit_cuts_to_width() {
        assert_eq!(fit("alice", 10), "alice");
        assert_eq!(fit("alexandra", 5), "alex…");
        assert_eq!(fit("日本語です", 5), "日本…");
    }
}
