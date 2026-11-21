//! ADR-031. A watch lasts for as long as its foreground command stays connected.

use std::mem::discriminant;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use termchat_core::proto::Ed25519Key;
use termchat_engine::{Engine, data::Status, engine};
use tokio::sync::mpsc;

use super::{Fail, Mark, err, last_mark, status_of};

const CLOCK_SKEW: u64 = 60;
const ALERT_WAIT: Duration = Duration::from_secs(60);
// ADR-034 minimums; debug builds keep one second so the end-to-end test stays fast.
const MIN_EVERY: u64 = if cfg!(debug_assertions) { 1 } else { 10 };
const MIN_AFTER: u64 = if cfg!(debug_assertions) { 1 } else { 30 };

struct Deadline {
    after: Duration,
    at: Instant,
    last: Option<u64>,
    alerted: bool,
}

impl Deadline {
    fn hear(&mut self, now: Instant, wall: u64, sent_at: u64) {
        let age = Duration::from_secs(wall.saturating_sub(sent_at));
        if sent_at.saturating_sub(wall) <= CLOCK_SKEW
            && self.last.is_none_or(|last| sent_at > last)
            && age < self.after
            && let Some(expires) = now.checked_add(self.after - age)
        {
            self.at = self.at.max(expires);
            self.last = Some(sent_at);
            self.alerted = false;
        }
    }

    fn due(&mut self, now: Instant) -> bool {
        if self.alerted || now < self.at {
            return false;
        }
        self.alerted = true;
        true
    }
}

enum Task {
    Send {
        every: Duration,
        next: Instant,
    },
    Monitor {
        owner: Ed25519Key,
        deadline: Deadline,
        pending: Vec<(Mark, Instant)>,
    },
}

pub(super) struct Watch {
    peer: Ed25519Key,
    task: Task,
    reply: mpsc::UnboundedSender<String>,
}

pub(super) fn is_request(request: &Value) -> bool {
    matches!(
        request["cmd"].as_str(),
        Some("watch_send" | "watch_monitor")
    )
}

pub(super) fn register(
    e: &Engine,
    request: &Value,
    reply: mpsc::UnboundedSender<String>,
    watches: &mut Vec<Watch>,
) {
    watches.retain(|w| !w.reply.is_closed());
    let result = (|| {
        let contact = |field: &str| {
            let name = request[field]
                .as_str()
                .ok_or_else(|| Fail(2, "watch needs a contact".into()))?;
            let i = e.find(name).map_err(|why| Fail(3, why))?;
            let key = e.data.contacts[i].key;
            e.verified_contact(&key).map_err(|why| Fail(5, why))?;
            Ok(key)
        };
        let peer = contact("peer")?;
        let now = Instant::now();
        let least = if request["cmd"] == "watch_send" {
            MIN_EVERY
        } else {
            MIN_AFTER
        };
        let bad_interval = || {
            Fail(
                2,
                format!("watch needs an interval of at least {least} whole seconds"),
            )
        };
        let seconds = request["seconds"]
            .as_u64()
            .filter(|&s| s >= least)
            .ok_or_else(bad_interval)?;
        let interval = Duration::from_secs(seconds);
        let at = now.checked_add(interval).ok_or_else(bad_interval)?;
        let task = if request["cmd"] == "watch_send" {
            Task::Send {
                every: interval,
                next: now,
            }
        } else {
            let owner = contact("owner")?;
            if owner == peer {
                return Err(Fail(
                    2,
                    "the watch owner and server must be different contacts".into(),
                ));
            }
            Task::Monitor {
                owner,
                deadline: Deadline {
                    after: interval,
                    at,
                    last: None,
                    alerted: false,
                },
                pending: Vec::new(),
            }
        };
        if watches
            .iter()
            .any(|w| w.peer == peer && discriminant(&w.task) == discriminant(&task))
        {
            return Err(Fail(2, "this watch is already running".into()));
        }
        Ok((peer, task))
    })();
    match result {
        Ok((peer, task)) => {
            if reply.send(json!({ "ready": true }).to_string()).is_ok() {
                watches.push(Watch { peer, task, reply });
            }
        }
        Err(f) => {
            let _ = reply.send(err(f));
        }
    }
}

pub(super) fn progress(e: &mut Engine, watches: &mut Vec<Watch>) {
    let heartbeat = e.take_heartbeat();
    let now = Instant::now();
    watches.retain_mut(|w| {
        if w.reply.is_closed() {
            return false;
        }
        if let Err(why) = w.step(e, now, heartbeat) {
            let _ = w.reply.send(err(Fail(5, why)));
            return false;
        }
        true
    });
}

impl Watch {
    fn step(
        &mut self,
        e: &mut Engine,
        now: Instant,
        heartbeat: Option<(Ed25519Key, u64)>,
    ) -> Result<(), String> {
        let peer = e.verified_contact(&self.peer)?;
        match &mut self.task {
            Task::Send { every, next } => {
                if now >= *next {
                    *next = now
                        .checked_add(*every)
                        .ok_or("watch interval is too long")?;
                    e.send_heartbeat(&self.peer)?;
                }
            }
            Task::Monitor {
                owner,
                deadline,
                pending,
            } => {
                let owner = e.verified_contact(owner)?;
                pending.retain(|&(mark, until)| match status_of(e, owner, mark) {
                    Some(Status::Sent) => false,
                    Some(Status::Failed(why)) => {
                        eprintln!("termchat watch: alert rejected: {why}");
                        false
                    }
                    Some(_) if now < until => true,
                    _ => {
                        eprintln!("termchat watch: alert delivery not confirmed");
                        false
                    }
                });
                if deadline.due(now) {
                    let contact = e.name(&e.data.contacts[peer]);
                    let seconds = deadline.after.as_secs();
                    let text = format!("{contact} has been silent for {seconds} seconds");
                    e.send(owner, &text)?;
                    eprintln!("termchat watch: outage detected; alert queued");
                    pending.push((last_mark(e, owner), now + ALERT_WAIT));
                }
                if let Some((key, sent_at)) = heartbeat
                    && key == self.peer
                {
                    deadline.hear(now, engine::now(), sent_at);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_pulses_get_only_the_time_they_have_left() {
        let start = Instant::now();
        let at = |s| start + Duration::from_secs(s);
        let mut timer = Deadline {
            after: Duration::from_secs(15),
            at: at(15),
            last: None,
            alerted: false,
        };
        timer.hear(at(10), 110, 105);
        assert_eq!(timer.at, at(20));
        for timestamp in [104, 105, 91, 182, u64::MAX] {
            timer.hear(at(19), 119, timestamp);
            assert_eq!(timer.at, at(20));
        }
        assert!(!timer.due(at(19)));
        assert!(timer.due(at(20)));
        assert!(!timer.due(at(50)));
        timer.hear(at(50), 150, 135);
        assert!(!timer.due(at(50)), "an expired pulse does not re-arm");
        timer.hear(at(51), 151, 153);
        assert_eq!(timer.at, at(66), "clock skew cannot extend the threshold");
        assert!(timer.due(at(66)));
        assert!(!timer.due(at(67)));
    }
}
