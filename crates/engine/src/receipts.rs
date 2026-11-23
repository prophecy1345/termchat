//! Delivery and read receipts (ADR-041): gathered per contact, sent together
//! after a random wait or right before our next message to them, never to a
//! stranger, a blocked contact or an older client, never in groups.

use super::*;
use crate::data::Seen;
use termchat_core::proto::MAX_RECEIPT_IDS;

/// The `Understands` level that reads receipts.
pub(super) const RECEIPT_LEVEL: u16 = 5;
/// Seconds a gathered receipt waits, at random within, to blur when it goes.
const WAIT_MIN: u64 = 5;
const WAIT_MAX: u64 = 30;

impl Engine {
    /// Contact `i` may get receipts from us.
    fn takes_receipts(&self, i: usize) -> bool {
        let c = &self.data.contacts[i];
        c.understands >= RECEIPT_LEVEL
            && !c.own
            && !c.blocked
            && !c.left
            && !self.is_request(&c.key)
    }

    /// A message or a whole document from contact `i` is on this device.
    pub(super) fn got(&mut self, i: usize, item: Seen) {
        if self.receipts && self.takes_receipts(i) {
            self.gather(i, item, false);
        }
    }

    fn gather(&mut self, i: usize, item: Seen, read: bool) {
        let r = &mut self.data.contacts[i].receipts;
        if read {
            // Read says delivered too.
            r.delivered.retain(|x| *x != item);
            r.read.push(item);
        } else {
            r.delivered.push(item);
        }
        if r.due.is_none() {
            let wait = getrandom::u64().unwrap_or(0) % (WAIT_MAX - WAIT_MIN + 1);
            r.due = Some(now() + WAIT_MIN + wait);
        }
    }

    /// The chat on screen: what came in it is read, if we say so (ADR-041:
    /// mutual, off by default).
    fn read_open(&mut self) {
        let Some(i) = self.open.and_then(|k| self.index(&k)) else {
            return;
        };
        if !self.read_receipts || !self.takes_receipts(i) {
            return;
        }
        let mut seen = Vec::new();
        for m in &mut self.data.contacts[i].messages {
            if m.mine || m.read || m.status != Status::Received {
                continue;
            }
            let item = match (&m.file, m.id) {
                (Some(f), _) if f.complete() => Seen::File(f.id),
                (None, Some(id)) => Seen::Message(id),
                _ => continue,
            };
            m.read = true;
            seen.push(item);
        }
        for item in seen {
            self.gather(i, item, true);
        }
    }

    /// Timers: read marks for the open chat, and receipts whose wait is over.
    pub(super) fn receipts_due(&mut self) {
        self.read_open();
        let t = now();
        for i in 0..self.data.people() {
            let due = self.data.contacts[i].receipts.due.is_some_and(|d| d <= t);
            if due && self.out_addr(i).is_some() && self.has_session(&self.data.contacts[i].key) {
                self.send_receipts(i);
                self.persist();
            }
        }
    }

    /// Everything gathered for contact `i`, now; what fails waits for the
    /// next try. Before our own messages, so the relay sees one burst.
    pub(super) fn send_receipts(&mut self, i: usize) {
        if !self.takes_receipts(i) {
            self.data.contacts[i].receipts = Default::default();
            return;
        }
        let key = self.data.contacts[i].key;
        for read in [false, true] {
            loop {
                let r = &mut self.data.contacts[i].receipts;
                let list = if read { &mut r.read } else { &mut r.delivered };
                if list.is_empty() {
                    break;
                }
                let n = list.len().min(MAX_RECEIPT_IDS);
                let batch: Vec<Seen> = list[..n].to_vec();
                let (mut messages, mut files) = (Vec::new(), Vec::new());
                for item in &batch {
                    match *item {
                        Seen::Message(id) => messages.push(id),
                        Seen::File(id) => files.push(id),
                    }
                }
                let body = Body::Receipt {
                    messages,
                    files,
                    read,
                };
                // To their devices only: a copy to ours would be no message
                // anyone sent (proto refuses it).
                if self.transmit_each(&key, body, false).is_err() {
                    return;
                }
                let r = &mut self.data.contacts[i].receipts;
                let list = if read { &mut r.read } else { &mut r.delivered };
                list.drain(..n);
            }
        }
        self.data.contacts[i].receipts.due = None;
    }

    /// Contact `i` says these of ours arrived, or were read.
    pub(super) fn take_receipt(&mut self, i: usize, messages: &[u64], files: &[u32], read: bool) {
        // Mutual: with ours off, theirs are not shown either.
        let read = read && self.read_receipts;
        for m in &mut self.data.contacts[i].messages {
            if !m.mine {
                continue;
            }
            let hit = match &m.file {
                Some(f) => files.contains(&f.id),
                None => m.id.is_some_and(|id| messages.contains(&id)),
            };
            if !hit {
                continue;
            }
            // Marks only go forward.
            if !m.delivered {
                m.delivered = true;
                m.changes.delivered = true;
            }
            if read && !m.read {
                m.read = true;
                m.changes.read = true;
            }
        }
    }
}
