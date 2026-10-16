//! Drawing. Every string that came from another person goes through `clean`.
//!
//! Look after herdr: the main area keeps the terminal's background, the theme
//! paints bars and windows, and whatever is under a window is dimmed.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};
use termchat_core::{fingerprint, safety_number};

use crate::app::{App, Main, Popup, Screen, Unlock, now};
use crate::data::{Contact, Status};
use crate::form::Form;
use crate::settings::Settings;
use crate::theme::Palette;

/// Contact list width, its right edge included.
const SIDEBAR: u16 = 28;
/// Below this the contact list is hidden; Up/Down still switch chats.
const NARROW: u16 = 64;

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
Ctrl+P       menu: backup, password, duress, Tor, theme, wipe
Ctrl+K       lock now (also after 5 min idle)
Ctrl+Q       quit
? / F1       this help

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
    match &app.screen {
        Screen::Setup(s) => form(f, t, s),
        Screen::Unlock(u) => unlock(f, t, u),
        Screen::Main(m) => main(f, t, m, &app.settings),
    }
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

/// A button: label and whether it is the primary action.
type Button<'a> = (&'a str, bool);

/// A centred window on the panel colour: body, then a row of buttons.
fn window(
    f: &mut Frame,
    t: &Palette,
    title: &str,
    mut body: Vec<Line<'_>>,
    buttons: &[Button],
    w: u16,
    danger: bool,
) {
    let edge = if danger { t.red } else { t.accent };
    if !buttons.is_empty() {
        let mut row = Vec::new();
        for (label, primary) in buttons {
            let style = match primary {
                true => Style::new()
                    .bg(edge)
                    .fg(on_accent(t))
                    .add_modifier(Modifier::BOLD),
                false => Style::new().bg(t.surface0).fg(t.text),
            };
            row.push(Span::styled(format!(" {label} "), style));
            row.push(Span::raw("  "));
        }
        body.push(Line::from(""));
        body.push(Line::from(row));
    }
    let w = w.min(f.area().width);
    let text = Paragraph::new(body).wrap(Wrap { trim: false });
    let rows = u16::try_from(text.line_count(w.saturating_sub(4))).unwrap_or(u16::MAX);
    let area = centered(f.area(), w, rows.saturating_add(2));
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
}

/// An input field, `w` columns wide: the tail of the value on the field colour.
fn field(t: &Palette, value: &str, focused: bool, w: usize) -> Line<'static> {
    let room = w.saturating_sub(1);
    let chars: Vec<char> = value.chars().collect();
    let mut shown: String = chars[chars.len().saturating_sub(room)..].iter().collect();
    if focused {
        shown.push('▏');
    }
    let pad = w.saturating_sub(Span::raw(&shown).width());
    Line::from(Span::styled(
        format!("{shown}{}", " ".repeat(pad)),
        Style::new().bg(t.surface0).fg(t.text),
    ))
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

/// Any form, centred, over whatever is below (setup screen or a settings popup).
fn form(f: &mut Frame, t: &Palette, form: &Form) {
    const W: u16 = 76;
    let mut lines: Vec<Line> = form
        .kind
        .info()
        .lines()
        .map(|l| Line::styled(l, Style::new().fg(t.subtext0)))
        .collect();
    for (i, ((label, secret), value)) in form.kind.labels().iter().zip(&form.fields).enumerate() {
        let shown = match secret {
            true => "•".repeat(value.chars().count()),
            false => value.to_string(),
        };
        let focused = i == form.field;
        let style = match focused {
            true => Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
            false => Style::new().fg(t.subtext0),
        };
        lines.push(Line::from(""));
        lines.push(Line::styled(*label, style));
        lines.push(field(t, &shown, focused, usize::from(W) - 4));
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
        crate::form::Kind::Setup => "^q quit",
        _ => "esc cancel",
    };
    window(
        f,
        t,
        form.kind.title().trim(),
        lines,
        &[
            ("↵ continue", true),
            ("tab next field", false),
            (leave, false),
        ],
        W,
        danger,
    );
}

fn unlock(f: &mut Frame, t: &Palette, u: &Unlock) {
    let shown = "•".repeat(u.input.chars().count());
    let mut lines = vec![
        Line::styled(
            "Password",
            Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        field(t, &shown, true, 46),
    ];
    if !u.error.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(u.error.as_str(), Style::new().fg(t.red)));
    }
    window(
        f,
        t,
        "termchat · locked",
        lines,
        &[("↵ unlock", true), ("^q quit", false)],
        50,
        false,
    );
}

fn main(f: &mut Frame, t: &Palette, m: &Main, settings: &Settings) {
    let area = f.area();
    let [body, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let chat = match area.width >= NARROW {
        true => {
            let [list, chat] =
                Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(1)]).areas(body);
            sidebar(f, t, m, list);
            chat
        }
        false => body,
    };
    chat_pane(f, t, m, chat);
    status_bar(f, t, m, bar);

    if let Some(popup) = &m.popup {
        f.buffer_mut()
            .set_style(area, Style::new().add_modifier(Modifier::DIM));
        popup_pane(f, t, m, popup, settings);
    }
}

fn sidebar(f: &mut Frame, t: &Palette, m: &Main, area: Rect) {
    let edge = Block::new()
        .borders(Borders::RIGHT)
        .border_style(Style::new().fg(t.surface_dim));
    let inner = edge.inner(area);
    f.render_widget(edge, area);
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
    }

    let [add, menu] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(foot);
    f.render_widget(Paragraph::new(Line::styled(" + add", muted)), add);
    f.render_widget(
        Paragraph::new(Line::styled("≡ menu ", muted)).alignment(Alignment::Right),
        menu,
    );
}

fn chat_pane(f: &mut Frame, t: &Palette, m: &Main, area: Rect) {
    let [head, history, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
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
        return input_bar(f, t, m, input);
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

    // Show the tail: count wrapped rows from the end until the pane is full.
    let pane = history.inner(ratatui::layout::Margin::new(1, 1));
    let height = usize::from(pane.height);
    let mut rows = 0;
    let mut start = lines.len();
    while start > 0 {
        let h = Paragraph::new(lines[start - 1].clone())
            .wrap(Wrap { trim: false })
            .line_count(pane.width)
            .max(1);
        if rows + h > height {
            break;
        }
        rows += h;
        start -= 1;
    }
    f.render_widget(
        Paragraph::new(lines[start..].to_vec()).wrap(Wrap { trim: false }),
        pane,
    );
    input_bar(f, t, m, input);
}

fn input_bar(f: &mut Frame, t: &Palette, m: &Main, area: Rect) {
    let style = Style::new().bg(t.surface0).fg(t.text);
    let prompt = Span::styled(
        " › ",
        Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
    );
    let row = Rect::new(area.x, area.y + 1, area.width, 1);
    let room = usize::from(area.width).saturating_sub(4);
    let line = match m.input.is_empty() {
        true => {
            let hint = match m.current() {
                Some(_) => "message",
                None => "add a contact first (Ctrl+A)",
            };
            Line::from(vec![
                prompt,
                Span::styled(hint, Style::new().fg(t.overlay0)),
            ])
        }
        false => {
            let chars: Vec<char> = m.input.chars().collect();
            let tail: String = chars[chars.len().saturating_sub(room)..].iter().collect();
            Line::from(vec![prompt, Span::raw(tail)])
        }
    };
    f.render_widget(Block::new().style(style), area);
    let x = match m.input.is_empty() {
        true => 3,
        false => u16::try_from(line.width()).unwrap_or(u16::MAX),
    };
    f.render_widget(Paragraph::new(line).style(style), row);
    if m.popup.is_none() {
        f.set_cursor_position(((row.x + x).min(row.right().saturating_sub(1)), row.y));
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
    let bar = Style::new().bg(t.panel_bg);
    f.render_widget(Block::new().style(bar), area);
    if left.width() + right.width() <= usize::from(area.width) {
        f.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
    }
    f.render_widget(Paragraph::new(left), area);
}

fn popup_pane(f: &mut Frame, t: &Palette, m: &Main, popup: &Popup, settings: &Settings) {
    let plain = |s: &str| -> Vec<Line> { s.lines().map(|l| Line::from(l.to_string())).collect() };
    let strong = Style::new().fg(t.accent).add_modifier(Modifier::BOLD);
    let close: &[Button] = &[("esc close", false)];
    match popup {
        Popup::Form(fm) => form(f, t, fm),
        Popup::Notice(text) => window(f, t, "note", plain(text), &[("↵ ok", true)], 70, false),
        Popup::Menu => {
            let menu = format!(
                "b  back up your identity\np  change password\nd  duress password\nt  Tor on/off\nc  colour theme: {}\nw  wipe everything from this device",
                settings.theme
            );
            let lines = keyed(t, &menu);
            window(f, t, "menu", lines, close, 50, false);
        }
        Popup::Help => window(f, t, "keys", keyed(t, HELP), close, 72, false),
        Popup::Add(s) => {
            let lines = vec![
                Line::from("Paste their invite and press Enter."),
                Line::from(""),
                field(t, s, true, 66),
            ];
            let buttons = [("↵ add", true), ("esc cancel", false)];
            window(f, t, "add contact", lines, &buttons, 70, false);
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
            let buttons = [("c copy to clipboard", true), ("esc close", false)];
            window(f, t, "your invite", lines, &buttons, 70, false);
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
            let buttons = [("v toggle verified", true), ("esc close", false)];
            window(f, t, "safety number", lines, &buttons, 64, false);
        }
        Popup::Rename(s) => {
            let lines = vec![
                Line::from("New name, only on this device. Empty resets it."),
                Line::from(""),
                field(t, s, true, 46),
            ];
            let buttons = [("↵ save", true), ("esc cancel", false)];
            window(f, t, "rename", lines, &buttons, 50, false);
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
            let buttons = [("y report and block", true), ("esc cancel", false)];
            window(
                f,
                t,
                "report to the relay operator",
                lines,
                &buttons,
                76,
                true,
            );
        }
        Popup::Delete => {
            let lines = plain("Delete this contact and the chat?");
            let buttons = [("y delete", true), ("esc keep", false)];
            window(f, t, "delete contact", lines, &buttons, 50, true);
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
    fn fit_cuts_to_width() {
        assert_eq!(fit("alice", 10), "alice");
        assert_eq!(fit("alexandra", 5), "alex…");
        assert_eq!(fit("日本語です", 5), "日本…");
    }
}
