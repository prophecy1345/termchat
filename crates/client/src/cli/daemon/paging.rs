//! ADR-040: pages that repeat until acknowledged, pass to the next person,
//! and say when the problem is gone. They live in the daemon's memory only.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use termchat_core::proto::{Ed25519Key, MAX_TEXT_BYTES};
use termchat_engine::Engine;
use tokio::sync::mpsc;

use super::{Fail, err, ok};

/// Open pages per daemon (ADR-040, section 3).
pub(super) const MAX_PAGES: usize = 64;
/// Contacts in one chain.
const MAX_CHAIN: usize = 8;
/// Reminders and escalation: at least a minute apart.
const MIN_STEP: u64 = 60;
/// A page gives up after a week at the latest.
const MAX_GIVE_UP: u64 = 7 * 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::cli) enum Outcome {
    /// Someone in the chain acknowledged it.
    Acknowledged,
    /// Opened without waiting for the outcome (the webhook receiver).
    Opened,
    /// Nobody acknowledged before it gave up.
    Unacknowledged,
    /// Resolved before anyone acknowledged it.
    Resolved,
    Invalid,
    UnknownContact,
    Ineligible,
    TooMany,
    SendFailed,
    Unavailable,
}

impl Outcome {
    fn send(self, reply: &mpsc::UnboundedSender<String>) {
        let _ = reply.send(ok(json!({ "page": self })));
    }

    pub(in crate::cli) fn result(self) -> Result<(), Fail> {
        let (code, why) = match self {
            Self::Acknowledged | Self::Opened => return Ok(()),
            Self::Unacknowledged => (4, "nobody acknowledged the page"),
            Self::Resolved => (4, "resolved before anyone acknowledged it"),
            Self::Invalid => (2, "invalid page request"),
            Self::UnknownContact => (3, "a contact was not found or is ambiguous"),
            Self::Ineligible => (
                5,
                "page needs the main device and accepted, verified contacts",
            ),
            Self::TooMany => (5, "too many open pages"),
            Self::SendFailed => (5, "the page could not be sent"),
            Self::Unavailable => (6, "the termchat daemon stopped or answered oddly"),
        };
        Err(Fail(code, why.into()))
    }
}

/// The CLI's reading of the daemon's one answer line.
pub(in crate::cli) fn result(line: &str) -> Result<(), Fail> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        ok: bool,
        page: Outcome,
    }
    serde_json::from_str::<Response>(line)
        .ok()
        .filter(|r| r.ok)
        .map_or(Outcome::Unavailable, |r| r.page)
        .result()
}

pub(super) struct Page {
    key: String,
    text: String,
    /// Pinned at start, every one verified then.
    chain: Vec<Ed25519Key>,
    /// One code per contact paged so far: `chain[..codes.len()]`.
    codes: Vec<String>,
    /// When each of them was paged: a reminder waits a full `every`.
    paged_at: Vec<Instant>,
    every: Duration,
    escalate: Duration,
    opened: Instant,
    until: Instant,
    next_remind: Instant,
    next_escalate: Instant,
    acked_by: Option<Ed25519Key>,
    firing: u32,
    waiters: Vec<mpsc::UnboundedSender<String>>,
}

pub(super) fn is_request(request: &Value) -> bool {
    request["cmd"] == "page" || request["cmd"] == "resolve"
}

pub(super) fn register(
    e: &mut Engine,
    request: &Value,
    reply: mpsc::UnboundedSender<String>,
    pages: &mut Vec<Page>,
) {
    if request["cmd"] == "resolve" {
        return resolve(e, request, &reply, pages);
    }
    let wait = request["wait"].as_bool().unwrap_or(true);
    let mut begin = || -> Result<Option<Page>, Outcome> {
        let text = request["text"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(Outcome::Invalid)?;
        let key = match request["key"].as_str() {
            Some(k) if !k.trim().is_empty() && k.len() <= 200 => k.to_string(),
            Some(_) => return Err(Outcome::Invalid),
            None => format!(
                "{:016x}",
                getrandom::u64().map_err(|_| Outcome::Unavailable)?
            ),
        };
        if let Some(p) = pages.iter_mut().find(|p| p.key == key) {
            p.firing += 1;
            if p.acked_by.is_none() {
                let note = format!("Still firing ({}): {}", p.firing, first_line(&p.text));
                for &who in p.paged() {
                    tell(e, who, &note);
                }
            }
            if wait {
                p.waiters.push(reply.clone());
            }
            return Ok(None);
        }
        if pages.len() >= MAX_PAGES {
            return Err(Outcome::TooMany);
        }
        let step = |name: &str, default: u64| match request[name].as_u64() {
            None => Ok(default),
            Some(n) if (MIN_STEP..=MAX_GIVE_UP).contains(&n) => Ok(n),
            Some(_) => Err(Outcome::Invalid),
        };
        let every = step("every", 300)?;
        let escalate = step("escalate", 900)?;
        let give_up = step("give_up", 7200)?;
        let names = request["to"].as_array().ok_or(Outcome::Invalid)?;
        if names.is_empty() || names.len() > MAX_CHAIN {
            return Err(Outcome::Invalid);
        }
        let mut chain = Vec::new();
        for name in names {
            let name = name.as_str().ok_or(Outcome::Invalid)?;
            let i = e.find(name).map_err(|_| Outcome::UnknownContact)?;
            let who = e.data.contacts[i].key;
            e.verified_contact(&who).map_err(|_| Outcome::Ineligible)?;
            if chain.contains(&who) {
                return Err(Outcome::Invalid);
            }
            chain.push(who);
        }
        let now = Instant::now();
        let after = |s: u64| {
            now.checked_add(Duration::from_secs(s))
                .ok_or(Outcome::Invalid)
        };
        let mut page = Page {
            key,
            text: text.to_string(),
            chain,
            codes: Vec::new(),
            paged_at: Vec::new(),
            every: Duration::from_secs(every),
            escalate: Duration::from_secs(escalate),
            opened: now,
            until: after(give_up)?,
            next_remind: after(every)?,
            next_escalate: after(escalate)?,
            acked_by: None,
            firing: 1,
            waiters: if wait {
                vec![reply.clone()]
            } else {
                Vec::new()
            },
        };
        page.page_next(e)?;
        Ok(Some(page))
    };
    match begin() {
        Ok(Some(page)) => {
            if !wait {
                Outcome::Opened.send(&reply);
            }
            pages.push(page);
        }
        Ok(None) => {
            if !wait {
                Outcome::Opened.send(&reply);
            }
        }
        Err(outcome) => outcome.send(&reply),
    }
}

fn resolve(
    e: &mut Engine,
    request: &Value,
    reply: &mpsc::UnboundedSender<String>,
    pages: &mut Vec<Page>,
) {
    let key = request["key"].as_str().unwrap_or_default();
    let Some(at) = pages.iter().position(|p| p.key == key) else {
        let _ = reply.send(err(Fail(3, "no open page with this key".into())));
        return;
    };
    let p = pages.remove(at);
    let note = format!(
        "Resolved after {}: {}",
        span(p.opened.elapsed()),
        first_line(&p.text)
    );
    for &who in p.paged() {
        tell(e, who, &note);
    }
    if p.acked_by.is_none() {
        for w in &p.waiters {
            Outcome::Resolved.send(w);
        }
    }
    let _ = reply.send(ok(json!({})));
}

/// `ack CODE` from a paged contact: that contact and the page.
pub(super) fn matching_ack(
    pages: &[Page],
    key: Ed25519Key,
    text: &str,
) -> Option<(Ed25519Key, String)> {
    let mut words = text.split_ascii_whitespace();
    let (Some(word), Some(code), None) = (words.next(), words.next(), words.next()) else {
        return None;
    };
    if !word.eq_ignore_ascii_case("ack") {
        return None;
    }
    pages.iter().find_map(|p| {
        let open = p.acked_by.is_none() && Instant::now() < p.until;
        let ours = p
            .codes
            .iter()
            .zip(&p.chain)
            .any(|(c, who)| *who == key && c.eq_ignore_ascii_case(code));
        (open && ours).then(|| (key, p.key.clone()))
    })
}

pub(super) fn progress(e: &mut Engine, pages: &mut Vec<Page>, ack: Option<(Ed25519Key, String)>) {
    let now = Instant::now();
    pages.retain_mut(|p| {
        if let Some((who, _)) = ack.as_ref().filter(|(_, k)| *k == p.key) {
            // Still a verified contact at the time of the decision (ADR-032).
            if let Ok(i) = e.verified_contact(who) {
                p.acked_by = Some(*who);
                let name = e.name(&e.data.contacts[i]);
                let first = first_line(&p.text).to_string();
                tell(e, *who, &format!("Acknowledged: {first}"));
                for &other in p.paged().iter().filter(|o| **o != *who) {
                    tell(e, other, &format!("Acknowledged by {name}: {first}"));
                }
                for w in p.waiters.drain(..) {
                    Outcome::Acknowledged.send(&w);
                }
            }
        }
        if now >= p.until {
            if p.acked_by.is_none() {
                let note = format!("Nobody acknowledged: {}", first_line(&p.text));
                for &who in p.paged() {
                    tell(e, who, &note);
                }
                for w in &p.waiters {
                    Outcome::Unacknowledged.send(w);
                }
            }
            return false;
        }
        if p.acked_by.is_some() {
            return true;
        }
        if now >= p.next_escalate && p.codes.len() < p.chain.len() {
            p.next_escalate = now + p.escalate;
            let before = p.codes.len();
            if p.page_next(e).is_ok() {
                let next = p.chain[before];
                if let Ok(i) = e.verified_contact(&next) {
                    let note = format!(
                        "Passed to {}: {}",
                        e.name(&e.data.contacts[i]),
                        first_line(&p.text)
                    );
                    for &who in &p.chain[..before] {
                        tell(e, who, &note);
                    }
                }
            }
        }
        if now >= p.next_remind {
            p.next_remind = now + p.every;
            let open = span(p.opened.elapsed());
            let due = p
                .paged_at
                .iter()
                .map(|at| now.duration_since(*at) >= p.every);
            for ((who, code), _) in p
                .chain
                .iter()
                .zip(&p.codes)
                .zip(due)
                .filter(|(_, due)| *due)
            {
                let note = format!(
                    "Not acknowledged after {open}: {}\nReply \"ack {code}\" to acknowledge.",
                    first_line(&p.text)
                );
                tell(e, *who, &note);
            }
        }
        true
    });
}

impl Page {
    /// Who has been paged so far.
    fn paged(&self) -> &[Ed25519Key] {
        &self.chain[..self.codes.len()]
    }

    /// Pages the next contact in the chain with a code of its own.
    fn page_next(&mut self, e: &mut Engine) -> Result<(), Outcome> {
        let who = *self.chain.get(self.codes.len()).ok_or(Outcome::Invalid)?;
        let i = e.verified_contact(&who).map_err(|_| Outcome::Ineligible)?;
        let code = format!(
            "{:016x}",
            getrandom::u64().map_err(|_| Outcome::Unavailable)?
        );
        let text = format!("{}\n\nReply \"ack {code}\" to acknowledge.", self.text);
        if text.len() > MAX_TEXT_BYTES {
            return Err(Outcome::Invalid);
        }
        e.send(i, &text).map_err(|_| Outcome::SendFailed)?;
        self.codes.push(code);
        self.paged_at.push(Instant::now());
        Ok(())
    }
}

/// A notice to a contact who is still verified; others get nothing.
fn tell(e: &mut Engine, who: Ed25519Key, text: &str) {
    if let Ok(i) = e.verified_contact(&who) {
        let _ = e.send(i, text);
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// "14m", "2h 5m", "40s".
fn span(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ if s % 3600 / 60 == 0 => format!("{}h", s / 3600),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(codes: &[&str], acked: bool) -> Page {
        let now = Instant::now();
        Page {
            key: "disk".into(),
            text: "Disk full\nmore".into(),
            chain: vec![[1; 32], [2; 32], [3; 32]],
            codes: codes.iter().map(|c| c.to_string()).collect(),
            paged_at: codes.iter().map(|_| now).collect(),
            every: Duration::from_secs(300),
            escalate: Duration::from_secs(900),
            opened: now,
            until: now + Duration::from_secs(3600),
            next_remind: now,
            next_escalate: now,
            acked_by: acked.then_some([1; 32]),
            firing: 1,
            waiters: Vec::new(),
        }
    }

    #[test]
    fn only_a_paged_contact_with_its_own_code_acknowledges() {
        let pages = [page(&["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"], false)];
        let ack = |key, text| matching_ack(&pages, key, text);
        assert_eq!(
            ack([1; 32], "ack aaaaaaaaaaaaaaaa"),
            Some(([1; 32], "disk".into()))
        );
        assert_eq!(
            ack([2; 32], " ACK BBBBBBBBBBBBBBBB\n"),
            Some(([2; 32], "disk".into()))
        );
        // Another's code, a contact not paged yet, a stranger, other words.
        assert_eq!(ack([1; 32], "ack bbbbbbbbbbbbbbbb"), None);
        assert_eq!(ack([3; 32], "ack aaaaaaaaaaaaaaaa"), None);
        assert_eq!(ack([9; 32], "ack aaaaaaaaaaaaaaaa"), None);
        for text in [
            "yes aaaaaaaaaaaaaaaa",
            "ack",
            "ack aaaaaaaaaaaaaaaa now",
            "ack aaaaaaaaaaaaaaab",
        ] {
            assert_eq!(ack([1; 32], text), None, "{text}");
        }
        let done = [page(&["aaaaaaaaaaaaaaaa"], true)];
        assert_eq!(matching_ack(&done, [1; 32], "ack aaaaaaaaaaaaaaaa"), None);
    }

    #[test]
    fn spans_read_naturally() {
        assert_eq!(span(Duration::from_secs(40)), "40s");
        assert_eq!(span(Duration::from_secs(14 * 60 + 5)), "14m");
        assert_eq!(span(Duration::from_secs(7200)), "2h");
        assert_eq!(span(Duration::from_secs(7500)), "2h 5m");
    }

    #[test]
    fn unexpected_socket_answers_never_count_as_acknowledged() {
        assert!(result(r#"{"ok":true,"page":"acknowledged"}"#).is_ok());
        assert!(matches!(
            result(r#"{"ok":true,"page":"unacknowledged"}"#),
            Err(Fail(4, _))
        ));
        for odd in [
            r#"{"ok":true}"#,
            r#"{"ok":false,"page":"acknowledged"}"#,
            "x",
        ] {
            assert!(matches!(result(odd), Err(Fail(6, _))), "{odd}");
        }
    }
}
