//! Drawing. Every string that came from another person goes through `clean`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use termchat_core::{fingerprint, safety_number};

use crate::app::{App, Main, Popup, Screen, Unlock, now};
use crate::data::{Contact, Status};
use crate::form::Form;

const HELP: &str = "\
Enter        send message
Up / Down    choose contact
Ctrl+A       add contact (paste their invite)
Ctrl+O       show your invite
Ctrl+F       safety number: verify the contact
Ctrl+Y       accept contact request
Ctrl+D       delete contact
Ctrl+B       block / unblock contact
Ctrl+R       rename contact (only on this device)
Ctrl+T       disappearing messages: off/5m/1h/1d/1w
Ctrl+E       no history: keep this chat in memory only
Ctrl+P       settings: backup, duress password, wipe
Ctrl+K       lock now (also after 5 min idle)
Ctrl+Q       quit
? / F1       this help

Marks:  ? request   ! not verified   × blocked
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

pub fn name(m: &Main, c: &Contact) -> String {
    let nick = m.core().nickname(&c.key).map(str::to_owned);
    let fp = fingerprint(&c.key);
    clean(
        &c.petname
            .clone()
            .or(nick)
            .unwrap_or_else(|| fp[..5].to_string()),
    )
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

pub fn render(f: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Setup(s) => form(f, s),
        Screen::Unlock(u) => unlock(f, u),
        Screen::Main(m) => main(f, m),
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

/// Any form, centred, over whatever is below (setup screen or a settings popup).
fn form(f: &mut Frame, form: &Form) {
    let mut lines: Vec<Line> = form
        .kind
        .info()
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    lines.push(Line::from(""));
    for (i, ((label, secret), value)) in form.kind.labels().iter().zip(&form.fields).enumerate() {
        let shown = match secret {
            true => "•".repeat(value.chars().count()),
            false => value.to_string(),
        };
        let style = if i == form.field {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new()
        };
        lines.push(Line::styled(format!("{label}:"), style));
        lines.push(Line::styled(
            format!("  {shown}{}", if i == form.field { "▏" } else { "" }),
            style,
        ));
    }
    if let Some(phrase) = &form.generated {
        lines.push(Line::from(""));
        lines.push(Line::from(
            format!("Generated: {}  (write it down)", phrase.as_str()).green(),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Tab: next field   Enter: continue   Esc: cancel   Ctrl+Q: quit".dim(),
    ));
    lines.push(Line::from(form.error.as_str().red()));
    let height = u16::try_from(lines.len()).unwrap_or(30) + 2;
    let area = centered(f.area(), 76, height);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(form.kind.title())),
        area,
    );
}

fn unlock(f: &mut Frame, u: &Unlock) {
    let lines = vec![
        Line::from("Password:"),
        Line::from(format!("  {}▏", "•".repeat(u.input.chars().count()))),
        Line::from(""),
        Line::from(u.error.as_str().red()),
    ];
    let area = centered(f.area(), 50, 6);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" termchat locked ")),
        area,
    );
}

fn main(f: &mut Frame, m: &Main) {
    let [top, input, status] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let [list, chat] = Layout::horizontal([Constraint::Length(26), Constraint::Min(10)]).areas(top);

    let items: Vec<ListItem> = m
        .data
        .contacts
        .iter()
        .map(|c| {
            let request = m.core().is_request(&c.key) == Some(true);
            let mark = match (request, c.verified) {
                _ if c.blocked => "× ",
                (true, _) => "? ",
                (false, false) => "! ",
                (false, true) => "  ",
            };
            let unread = if c.unread > 0 {
                format!(" ({})", c.unread)
            } else {
                String::new()
            };
            ListItem::new(format!("{mark}{}{unread}", name(m, c)))
        })
        .collect();
    let mut state = ListState::default().with_selected((!items.is_empty()).then_some(m.selected));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" contacts "))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        list,
        &mut state,
    );

    chat_pane(f, m, chat);

    f.render_widget(
        Paragraph::new(m.input.as_str()).block(Block::bordered().title(" message ")),
        input,
    );
    let x = input.x + 1 + u16::try_from(Line::from(m.input.as_str()).width()).unwrap_or(u16::MAX);
    f.set_cursor_position((x.min(input.right().saturating_sub(2)), input.y + 1));

    let link = if m.connected {
        "● online".green()
    } else {
        "○ connecting…".yellow()
    };
    let line = Line::from(vec![
        link,
        Span::raw(format!("  {}  ", clean(&m.data.nickname))),
        Span::raw("? help  ").dim(),
        Span::raw(if m.data.backed_up {
            ""
        } else {
            "! no backup: Ctrl+P  "
        })
        .yellow(),
        Span::raw(m.status.as_str()).red(),
    ]);
    f.render_widget(Paragraph::new(line), status);

    if let Some(popup) = &m.popup {
        popup_pane(f, m, popup);
    }
}

fn chat_pane(f: &mut Frame, m: &Main, area: Rect) {
    let Some(c) = m.current() else {
        let text = "No contacts yet.\n\nCtrl+O shows your invite: send it to a friend.\nCtrl+A adds a friend from their invite.";
        f.render_widget(Paragraph::new(text).block(Block::bordered()), area);
        return;
    };
    let mut title = format!(" {} ", name(m, c));
    if !c.verified {
        title.push_str("! not verified (Ctrl+F) ");
    }
    if let Some(t) = c.expires_in {
        title.push_str(&format!("⏱ {} ", span(u64::from(t))));
    }
    if c.no_history {
        title.push_str("no history ");
    }

    let them = name(m, c);
    let lines: Vec<Line> = c
        .messages
        .iter()
        .map(|msg| {
            let text = clean(&msg.text);
            match &msg.status {
                Status::System => Line::from(format!("  {text}").dim().italic()),
                status => {
                    let mark = match status {
                        Status::Queued | Status::Sending(_) => " …",
                        Status::Sent => " ✓",
                        Status::Failed(_) => " ✗",
                        _ => "",
                    };
                    let who = if msg.mine {
                        Span::raw("you").cyan()
                    } else {
                        Span::raw(them.clone()).magenta()
                    };
                    let mut spans = vec![
                        Span::raw(format!("{:>4} ", age(msg.at))).dim(),
                        who,
                        Span::raw(format!(": {text}")),
                    ];
                    spans.push(Span::raw(mark).dim());
                    if let Status::Failed(why) = status {
                        spans.push(Span::raw(format!(" ({why})")).red());
                    }
                    Line::from(spans)
                }
            }
        })
        .collect();

    // Show the tail: count wrapped rows from the end until the pane is full.
    let width = usize::from(area.width.saturating_sub(2)).max(1);
    let height = usize::from(area.height.saturating_sub(2));
    let mut rows = 0;
    let mut start = lines.len();
    while start > 0 {
        let h = lines[start - 1].width().div_ceil(width).max(1);
        if rows + h > height {
            break;
        }
        rows += h;
        start -= 1;
    }
    let tail: Vec<Line> = lines[start..].to_vec();
    f.render_widget(
        Paragraph::new(tail)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title)),
        area,
    );
}

fn popup_pane(f: &mut Frame, m: &Main, popup: &Popup) {
    let (title, text, w, h) = match popup {
        Popup::Form(fm) => return form(f, fm),
        Popup::Notice(text) => (" note ", format!("{text}\n\nany key: close"), 70, 7),
        Popup::Menu => (
            " settings ",
            "b  back up your identity\nd  duress password\nw  wipe everything from this device\n\nany other key: close".to_string(),
            48,
            8,
        ),
        Popup::Help => (" help ", HELP.to_string(), 70, 30),
        Popup::Add(s) => (
            " add contact ",
            format!("Paste their invite and press Enter (Esc cancels):\n\n{s}▏"),
            70,
            10,
        ),
        Popup::Invite(invite) => {
            let fp = fingerprint(&m.me());
            (
                " your invite ",
                format!(
                    "Send this to a friend. It holds only public keys.\n\n{invite}\n\nYour fingerprint: {fp}\n\nc: copy to clipboard   any other key: close"
                ),
                70,
                14,
            )
        }
        Popup::Verify => {
            let Some(c) = m.current() else { return };
            let number = safety_number(&m.me(), &c.key);
            let groups: Vec<&str> = number.split(' ').collect();
            let rows: Vec<String> = groups.chunks(4).map(|g| g.join(" ")).collect();
            let state = if c.verified {
                "VERIFIED"
            } else {
                "not verified"
            };
            (
                " safety number ",
                format!(
                    "Compare with {} in person or by phone.\nSame numbers on both screens = nobody is in the middle.\n\n{}\n\nNow: {state}\nv: toggle verified   any other key: close",
                    name(m, c),
                    rows.join("\n")
                ),
                64,
                12,
            )
        }
        Popup::Rename(s) => (
            " rename (only on this device) ",
            format!("New name, Enter to save, empty to reset:\n\n{s}▏"),
            50,
            7,
        ),
        Popup::Delete => (
            " delete contact ",
            "Delete this contact and the chat? y: yes   any other key: no".to_string(),
            50,
            5,
        ),
    };
    let area = centered(f.area(), w, h);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_escapes_and_bidi() {
        assert_eq!(clean("hi\x1b[2Jthere\u{202E}lol\nok"), "hi[2Jtherelol ok");
        assert_eq!(clean("Ёжик 🦔"), "Ёжик 🦔");
    }
}
