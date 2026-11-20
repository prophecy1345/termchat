//! ADR-032: the reply authorizes one pending question on this socket.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use termchat_core::proto::{Ed25519Key, MAX_TEXT_BYTES};
use termchat_engine::{Engine, data::Status};
use tokio::sync::mpsc;

use super::{Fail, Mark, last_mark, ok, status_of};

const UNAVAILABLE: &str = "the termchat daemon stopped or returned an invalid approval result";

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Approved,
    Declined,
    Invalid,
    UnknownContact,
    TimedOut,
    Ineligible,
    Busy,
    SendFailed,
    Unavailable,
}

impl Outcome {
    fn send(self, reply: &mpsc::UnboundedSender<String>) {
        let _ = reply.send(ok(json!({ "approval": self })));
    }

    fn result(self) -> Result<(), Fail> {
        let (code, why) = match self {
            Self::Approved => return Ok(()),
            Self::Declined => (1, "approval declined"),
            Self::Invalid => (2, "invalid approval request"),
            Self::UnknownContact => (3, "contact not found or ambiguous"),
            Self::TimedOut => (4, "no valid approval reply in time"),
            Self::Ineligible => (
                5,
                "ask needs the main device and an accepted, verified contact",
            ),
            Self::Busy => (5, "an approval request is already pending for this contact"),
            Self::SendFailed => (5, "approval question could not be sent"),
            Self::Unavailable => (6, UNAVAILABLE),
        };
        Err(Fail(code, why.into()))
    }
}

pub(super) fn result(line: &str) -> Result<(), Fail> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        ok: bool,
        approval: Outcome,
    }
    serde_json::from_str::<Response>(line)
        .ok()
        .filter(|r| r.ok)
        .map_or(Outcome::Unavailable, |r| r.approval)
        .result()
}

pub(super) struct Approval {
    key: Ed25519Key,
    code: String,
    until: Instant,
    sent: Option<Mark>,
    reply: mpsc::UnboundedSender<String>,
}

pub(super) fn register(
    e: &mut Engine,
    request: &Value,
    reply: mpsc::UnboundedSender<String>,
    pending: &mut Vec<Approval>,
) {
    if reply.is_closed() {
        return;
    }
    progress(e, pending, None);
    let mut begin = || -> Result<Approval, Outcome> {
        let field = |key| {
            request[key]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or(Outcome::Invalid)
        };
        let query = field("to")?;
        let question = field("text")?;
        let timeout = request["timeout"]
            .as_u64()
            .filter(|&n| n > 0)
            .ok_or(Outcome::Invalid)?;
        let i = e.find(query).map_err(|_| Outcome::UnknownContact)?;
        let key = e.data.contacts[i].key;
        e.verified_contact(&key).map_err(|_| Outcome::Ineligible)?;
        if pending.iter().any(|p| p.key == key) {
            return Err(Outcome::Busy);
        }
        let code = format!(
            "{:016x}",
            getrandom::u64().map_err(|_| Outcome::Unavailable)?
        );
        let text =
            format!("{question}\n\nTo approve, send: yes {code}\nTo decline, send: no {code}");
        if text.len() > MAX_TEXT_BYTES {
            return Err(Outcome::Invalid);
        }
        let until = Instant::now()
            .checked_add(Duration::from_secs(timeout))
            .ok_or(Outcome::Invalid)?;
        e.send(i, &text).map_err(|_| Outcome::Unavailable)?;
        Ok(Approval {
            key,
            code,
            until,
            sent: Some(last_mark(e, i)),
            reply: reply.clone(),
        })
    };
    match begin() {
        Ok(approval) => pending.push(approval),
        Err(outcome) => outcome.send(&reply),
    }
}

pub(super) fn matching_reply(
    pending: &[Approval],
    key: Ed25519Key,
    text: &str,
) -> Option<(Ed25519Key, bool)> {
    let p = pending
        .iter()
        .find(|p| p.key == key && !p.reply.is_closed() && Instant::now() < p.until)?;
    let (decision, code) = text.split_once(' ')?;
    if code != p.code {
        return None;
    }
    match decision {
        "yes" => Some((key, true)),
        "no" => Some((key, false)),
        _ => None,
    }
}

pub(super) fn progress(
    e: &Engine,
    pending: &mut Vec<Approval>,
    answer: Option<(Ed25519Key, bool)>,
) {
    pending.retain_mut(|p| {
        if p.reply.is_closed() {
            return false;
        }
        let outcome = match e.verified_contact(&p.key) {
            Err(_) => Some(Outcome::Ineligible),
            Ok(i) => p.check(e, i, answer),
        };
        if let Some(outcome) = outcome {
            outcome.send(&p.reply);
        }
        outcome.is_none()
    });
}

impl Approval {
    fn check(
        &mut self,
        e: &Engine,
        i: usize,
        answer: Option<(Ed25519Key, bool)>,
    ) -> Option<Outcome> {
        if let Some(mark) = self.sent {
            match status_of(e, i, mark) {
                Some(Status::Failed(_)) => return Some(Outcome::SendFailed),
                Some(Status::Sent) | None => self.sent = None,
                _ => {}
            }
        }
        if Instant::now() >= self.until {
            return Some(Outcome::TimedOut);
        }
        let (_, yes) = answer.filter(|(key, _)| *key == self.key)?;
        Some(if yes {
            Outcome::Approved
        } else {
            Outcome::Declined
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_stops_at_the_deadline_or_disconnect() {
        let (reply, receiver) = mpsc::unbounded_channel();
        let key = [1; 32];
        let mut pending = [Approval {
            key,
            code: "0123456789abcdef".into(),
            until: Instant::now() + Duration::from_secs(60),
            sent: None,
            reply,
        }];
        let yes = "yes 0123456789abcdef";
        assert_eq!(matching_reply(&pending, key, yes), Some((key, true)));
        assert_eq!(
            matching_reply(&pending, key, "no 0123456789abcdef"),
            Some((key, false))
        );
        for text in [
            "YES 0123456789abcdef",
            "yes  0123456789abcdef",
            "yes\t0123456789abcdef",
        ] {
            assert_eq!(matching_reply(&pending, key, text), None);
        }
        drop(receiver);
        assert_eq!(matching_reply(&pending, key, yes), None);
        let (reply, _receiver) = mpsc::unbounded_channel();
        pending[0].reply = reply;
        pending[0].until = Instant::now();
        assert_eq!(matching_reply(&pending, key, yes), None);
    }

    #[test]
    fn unexpected_socket_responses_cannot_approve_or_decline() {
        assert!(result(r#"{"ok":true,"approval":"approved"}"#).is_ok());
        assert!(matches!(
            result(r#"{"ok":true,"approval":"declined"}"#),
            Err(Fail(1, _))
        ));
        for unexpected in [
            r#"{"ok":true}"#,
            r#"{"ok":false,"code":1,"error":"old daemon error"}"#,
            r#"{"ok":false,"approval":"approved"}"#,
            r#"{"ok":true,"approval":"declined","approval":"approved"}"#,
            r#"{"ok":true,"approval":"approved","code":5}"#,
            "invalid",
        ] {
            assert!(
                matches!(result(unexpected), Err(Fail(6, _))),
                "{unexpected}"
            );
        }
    }
}
