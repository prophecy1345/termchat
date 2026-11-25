//! Documents of more than 1 MiB (ADR-043): offered first, sent part by part
//! only after the receiver asks, at most `LARGE_WINDOW` parts unacknowledged.
//! Both sides keep the bytes in a spool file of sealed records, never in the
//! state file. Between two people only: never in groups, never copied.

use std::io::{self, Read, Seek, SeekFrom, Write};

use super::*;
use crate::data::{Large, Seen};
use termchat_core::proto::{
    LARGE_CHUNK, LARGE_FILES_LEVEL, LARGE_WINDOW, MAX_LARGE_FILE_BYTES, large_chunk_len,
    large_chunks,
};
use termchat_core::{RECORD_OVERHEAD, open_record, seal_record};
use zeroize::Zeroizing;

/// One sealed part in the spool; the last one is shorter.
const RECORD: usize = LARGE_CHUNK + RECORD_OVERHEAD;
/// The receiver confirms after this many parts, and after the last.
const ACK_EVERY: u16 = 16;
/// Parts go in a burst of this many, then one per `LARGE_EVERY` seconds,
/// below the relay's four a second for parts (ADR-043 amendment 1).
pub(super) const LARGE_BURST: f64 = 10.0;
const LARGE_EVERY: f64 = 0.3;
/// A relay before `FAST_RELAY` counts parts with text at one a second:
/// there a part goes at most this often, so texts still fit.
const SLOW_EVERY: Duration = Duration::from_millis(1500);
const FAST_RELAY: (u32, u32, u32) = (0, 30, 0);
/// ADR-047: from this relay on, parts and the chunks of small documents
/// share a budget of ten a second; the client keeps below it.
const FASTER_RELAY: (u32, u32, u32) = (0, 34, 0);
pub(super) const FASTER_BURST: f64 = 30.0;
pub(super) const FASTER_EVERY: f64 = 0.12;
/// An offer nobody answered is dropped on both sides.
const OFFER_TTL: u64 = 7 * 86_400;
/// The sender's offers waiting or moving, and their bytes, at once.
const MAX_OFFERS: usize = 4;
const MAX_SPOOLED_MINE: usize = 100 << 20;
/// Received documents asked for and not saved yet.
const MAX_SPOOLED_THEIRS: usize = 64 << 20;
/// The first relay version that takes parts in 72 KiB frames.
const LARGE_RELAY: (u32, u32, u32) = (0, 29, 0);

fn aad(id: u32, index: u16) -> [u8; 6] {
    let mut a = [0; 6];
    a[..4].copy_from_slice(&id.to_le_bytes());
    a[4..].copy_from_slice(&index.to_le_bytes());
    a
}

/// Adds what `budget` earned since it was last counted, up to `burst`.
pub(super) fn refill(budget: &mut (f64, Instant), every: f64, burst: f64) {
    budget.0 = (budget.0 + budget.1.elapsed().as_secs_f64() / every).min(burst);
    budget.1 = Instant::now();
}

impl Engine {
    /// ADR-047: every queue a document to contact `i` goes into is on a relay
    /// that paces its chunks with parts, so they may share the parts' budget.
    pub(super) fn faster(&self, i: usize) -> bool {
        let key = self.data.contacts[i].key;
        let addrs = self.out_addr(i).into_iter().cloned();
        let mut addrs = addrs.chain(self.copies_of(i).into_iter().map(|(_, a)| a));
        addrs.all(|a| {
            self.live(&key, &a.relay_url)
                .and_then(|l| l.version)
                .is_some_and(|v| v >= FASTER_RELAY)
        })
    }

    fn spool_path(&self, name: &str) -> PathBuf {
        data::spool_dir(&self.path).join(name)
    }

    /// Spool files no message needs any more go.
    pub(super) fn sweep_spool(&self) {
        data::sweep_spool(&self.path, &self.data.spool_names());
    }

    /// ADR-043: a document of more than 1 MiB to contact `i`, sealed into the
    /// spool now and offered once a session is there.
    pub(super) fn offer_file(&mut self, i: usize, name: &str, bytes: &[u8]) -> Result<(), String> {
        // Their answers reach our main device only.
        if !self.is_primary() {
            return Err("send documents over 1 MiB from your main device".into());
        }
        let c = &self.data.contacts[i];
        if c.understands < LARGE_FILES_LEVEL {
            return Err("their app takes documents up to 1 MiB".into());
        }
        let size = u32::try_from(bytes.len())
            .ok()
            .filter(|&n| n <= MAX_LARGE_FILE_BYTES)
            .ok_or("documents are limited to 25 MiB")?;
        let mine = self
            .data
            .contacts
            .iter()
            .flat_map(|c| &c.messages)
            .filter(|m| m.mine)
            .filter_map(|m| m.file.as_ref())
            .filter(|f| f.large.as_ref().is_some_and(|l| l.held));
        let (count, spooled) = mine.fold((0, 0), |(n, b), f| (n + 1, b + f.size as usize));
        if count >= MAX_OFFERS || spooled + bytes.len() > MAX_SPOOLED_MINE {
            return Err("too many large documents waiting to be taken".into());
        }
        let mut id = [0; 4];
        getrandom::fill(&mut id).map_err(|e| e.to_string())?;
        let id = u32::from_le_bytes(id);
        let mut spool = [0; 12];
        getrandom::fill(&mut spool).map_err(|e| e.to_string())?;
        let spool: String = spool.iter().map(|b| format!("{b:02x}")).collect();
        let mut sealed = Vec::with_capacity(bytes.len() + large_chunks(size) * RECORD_OVERHEAD);
        for (index, part) in bytes.chunks(LARGE_CHUNK).enumerate() {
            let index = u16::try_from(index).map_err(|_| "too many parts")?;
            let record =
                seal_record(&self.key, &aad(id, index), part).map_err(|e| e.to_string())?;
            sealed.extend_from_slice(&record);
        }
        let path = self.spool_path(&spool);
        data::create_private(&path, true)
            .and_then(|mut f| f.write_all(&sealed).and_then(|()| f.sync_all()))
            .map_err(|e| format!("cannot keep the document: {e}"))?;
        let expires_in = self.data.contacts[i].expires_in;
        let at = now();
        self.data.contacts[i].messages.push(Msg {
            mine: true,
            text: name.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
            file: Some(FileMsg {
                id,
                name: name.to_string(),
                size,
                done: 0,
                picture: false,
                data: None,
                large: Some(Large {
                    held: true,
                    spool,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        });
        Ok(())
    }

    /// Offers waiting to go out and parts the receiver asked for, as the
    /// budget and the window allow. `true` if anything went.
    pub(super) fn send_parts(&mut self) -> bool {
        let mut sent = false;
        for i in 0..self.data.contacts.len() {
            let c = &self.data.contacts[i];
            let key = c.key;
            if c.blocked || c.left || !self.has_session(&key) {
                continue;
            }
            let ours = |m: &Msg| {
                m.mine
                    && m.file
                        .as_ref()
                        .and_then(|f| f.large.as_ref())
                        .is_some_and(|l| l.held && !l.stopped)
            };
            // Every offer not out yet goes; then parts of the one asked for.
            let queued: Vec<usize> = (0..c.messages.len())
                .filter(|&m| ours(&c.messages[m]) && c.messages[m].status == Status::Queued)
                .collect();
            for m in queued {
                sent |= self.send_offer(i, m);
            }
            let wanted = |m: &Msg| {
                ours(m)
                    && m.file
                        .as_ref()
                        .and_then(|f| f.large.as_ref())
                        .is_some_and(|l| l.wanted)
            };
            let Some(m) = self.data.contacts[i].messages.iter().position(wanted) else {
                continue;
            };
            let relay = self.out_addr(i).map(|a| a.relay_url.clone());
            let version = relay
                .and_then(|r| self.live(&key, &r))
                .and_then(|l| l.version);
            let fast = version.is_some_and(|v| v >= FAST_RELAY);
            match version.is_some_and(|v| v >= FASTER_RELAY) {
                true => refill(&mut self.large_budget, FASTER_EVERY, FASTER_BURST),
                false => refill(&mut self.large_budget, LARGE_EVERY, LARGE_BURST),
            }
            while self.large_budget.0 >= 1.0 {
                if !fast && self.slow_part_at.elapsed() < SLOW_EVERY {
                    break;
                }
                let msg = &self.data.contacts[i].messages[m];
                let Some(f) = msg.file.as_ref() else { break };
                let Some(l) = f.large.as_ref() else { break };
                if !l.wanted || f.complete() || f.done - l.acked >= LARGE_WINDOW {
                    break;
                }
                let (id, index, spool) = (f.id, f.done, l.spool.clone());
                let data = match self.read_part(&spool, id, index, f.size) {
                    Ok(data) => data,
                    Err(e) => {
                        self.stop_large(i, m, true, Some(e));
                        break;
                    }
                };
                // Counted before the send, so a restart never repeats a part.
                if let Some(f) = self.data.contacts[i].messages[m].file.as_mut() {
                    f.done += 1;
                }
                self.large_budget.0 -= 1.0;
                self.slow_part_at = Instant::now();
                sent = true;
                let body = Body::FilePart {
                    id,
                    index,
                    data: data.to_vec(),
                };
                match self.transmit(&key, body) {
                    Ok(client_id) => {
                        self.chunks.insert(client_id, key);
                        self.parts_out.insert(client_id, (key, id, index));
                    }
                    Err(e) => {
                        self.stop_large(i, m, true, Some(e));
                        break;
                    }
                }
            }
        }
        sent
    }

    /// The offer itself, once their relay is known to take parts.
    fn send_offer(&mut self, i: usize, m: usize) -> bool {
        let key = self.data.contacts[i].key;
        let relay = self.out_addr(i).map(|a| a.relay_url.clone());
        let Some(live) = relay.and_then(|r| self.live(&key, &r)) else {
            return false;
        };
        if !live.connected {
            return false;
        }
        if live.version.is_none_or(|v| v < LARGE_RELAY) {
            let why = "their relay does not take documents over 1 MiB".to_string();
            self.stop_large(i, m, false, Some(why));
            return true;
        }
        let expires_in = self.data.contacts[i].expires_in;
        let Some(f) = self.data.contacts[i].messages[m].file.as_ref() else {
            return false;
        };
        let body = Body::FileOffer {
            id: f.id,
            name: f.name.clone(),
            size: f.size,
            expires_in,
        };
        let status = match self.transmit(&key, body) {
            Ok(client_id) => Status::Sending(client_id),
            Err(e) => {
                self.stop_large(i, m, false, Some(e.clone()));
                Status::Failed(e)
            }
        };
        self.data.contacts[i].messages[m].status = status;
        true
    }

    fn read_part(
        &self,
        spool: &str,
        id: u32,
        index: u16,
        size: u32,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        let len = large_chunk_len(size, index).ok_or("no such part")? + RECORD_OVERHEAD;
        let mut record = vec![0; len];
        let mut file = std::fs::File::open(self.spool_path(spool)).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start((usize::from(index) * RECORD) as u64))
            .and_then(|_| file.read_exact(&mut record))
            .map_err(|e| format!("the document's copy is damaged: {e}"))?;
        open_record(&self.key, &aad(id, index), &record)
            .map_err(|_| "the document's copy is damaged".into())
    }

    /// Ends transfer `m` of contact `i` for good; `tell`: the contact hears.
    pub(super) fn stop_large(&mut self, i: usize, m: usize, tell: bool, why: Option<String>) {
        let key = self.data.contacts[i].key;
        let Some(f) = self.data.contacts[i].messages[m].file.as_mut() else {
            return;
        };
        let id = f.id;
        if let Some(l) = f.large.as_mut() {
            l.stopped = true;
            l.held = false;
        }
        if let Some(why) = why {
            self.data.contacts[i].messages[m].status = Status::Failed(why);
        }
        if tell {
            let _ = self.transmit(&key, Body::FileStop { id });
        }
        self.persist();
        self.sweep_spool();
    }

    /// Message `m` of contact `i` is the large document `id` of the given side.
    fn find_large(&self, i: usize, id: u32, mine: bool) -> Option<usize> {
        self.data.contacts[i].messages.iter().position(|m| {
            m.mine == mine
                && m.file
                    .as_ref()
                    .is_some_and(|f| f.id == id && f.large.is_some())
        })
    }

    /// Offers, requests, acknowledgements, cancels and parts from contact `i`.
    pub(super) fn take_large(&mut self, i: usize, body: Body) {
        match body {
            Body::FileOffer {
                id,
                name,
                size,
                expires_in,
            } => {
                if self.find_large(i, id, false).is_some() {
                    return;
                }
                let at = now();
                self.data.contacts[i].messages.push(Msg {
                    mine: false,
                    text: name.clone(),
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Received,
                    file: Some(FileMsg {
                        id,
                        name,
                        size,
                        done: 0,
                        picture: false,
                        data: None,
                        large: Some(Large::default()),
                    }),
                    ..Default::default()
                });
                let key = self.data.contacts[i].key;
                self.happened.push(Happened::Alert {
                    key,
                    request: false,
                });
            }
            Body::FileWant { id } => {
                let Some(m) = self.find_large(i, id, true) else {
                    return;
                };
                if let Some(l) = self.data.contacts[i].messages[m]
                    .file
                    .as_mut()
                    .and_then(|f| f.large.as_mut())
                    .filter(|l| l.held && !l.stopped)
                {
                    l.wanted = true;
                }
            }
            Body::FileAck { id, next } => {
                let Some(m) = self.find_large(i, id, true) else {
                    return;
                };
                let Some(f) = self.data.contacts[i].messages[m].file.as_mut() else {
                    return;
                };
                let (done, whole) = (f.done, f.parts());
                if let Some(l) = f.large.as_mut() {
                    l.acked = l.acked.max(next.min(done));
                    if usize::from(l.acked) == whole {
                        l.held = false;
                        self.sweep_spool();
                    }
                }
            }
            Body::FileStop { id } => {
                for mine in [true, false] {
                    if let Some(m) = self.find_large(i, id, mine) {
                        let whole = self.data.contacts[i].messages[m]
                            .file
                            .as_ref()
                            .is_some_and(|f| !mine && f.complete());
                        if !whole {
                            self.stop_large(i, m, false, None);
                        }
                    }
                }
            }
            Body::FilePart { id, index, data } => self.take_part(i, id, index, &data),
            _ => {}
        }
    }

    /// Audit 2026-11-25, finding 4: frames still in memory are lost when the
    /// link drops or the app stops. On each connection to their relay our
    /// parts go again from the last one they confirmed (they drop the ones
    /// they have), and our ask or our progress goes again.
    pub(super) fn resume_large(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        if !self.has_session(&key) {
            return;
        }
        let mut again = Vec::new();
        for m in &mut self.data.contacts[i].messages {
            let mine = m.mine;
            let Some(f) = m.file.as_mut() else { continue };
            let Some((acked, accepted)) = f
                .large
                .as_ref()
                .filter(|l| l.wanted && l.held && !l.stopped)
                .map(|l| (l.acked, l.accepted))
            else {
                continue;
            };
            match mine {
                true => f.done = f.done.min(acked.max(accepted)),
                false if f.done == 0 => again.push(Body::FileWant { id: f.id }),
                false if !f.complete() => again.push(Body::FileAck {
                    id: f.id,
                    next: f.done,
                }),
                false => {}
            }
        }
        // Answers to frames lost with the link never come.
        self.parts_out.retain(|_, (k, ..)| *k != key);
        self.persist();
        for body in again {
            let _ = self.transmit(&key, body);
        }
        self.send_chunks();
    }

    /// The relay took part `index` of our document `id` to `key`.
    pub(super) fn part_accepted(&mut self, key: &Ed25519Key, id: u32, index: u16) {
        let Some(i) = self.index(key) else { return };
        let Some(m) = self.find_large(i, id, true) else {
            return;
        };
        if let Some(l) = self.data.contacts[i].messages[m]
            .file
            .as_mut()
            .and_then(|f| f.large.as_mut())
        {
            l.accepted = l.accepted.max(index + 1);
        }
    }

    fn take_part(&mut self, i: usize, id: u32, index: u16, data: &[u8]) {
        let Some(m) = self.find_large(i, id, false) else {
            return;
        };
        let Some(f) = self.data.contacts[i].messages[m].file.as_ref() else {
            return;
        };
        let Some(l) = f
            .large
            .as_ref()
            .filter(|l| l.wanted && l.held && !l.stopped)
        else {
            return;
        };
        // A part seen before a restart, delivered again: already in the spool.
        if index < f.done {
            return;
        }
        let (size, done, spool) = (f.size, f.done, l.spool.clone());
        if index != done || large_chunk_len(size, index) != Some(data.len()) {
            let broken = format!("{} did not arrive whole", f.name);
            self.data.contacts[i].messages[m].text = broken;
            return self.stop_large(i, m, true, None);
        }
        if let Err(e) = self.write_part(&spool, id, index, data) {
            return self.stop_large(i, m, true, Some(format!("cannot keep the document: {e}")));
        }
        let key = self.data.contacts[i].key;
        let Some(f) = self.data.contacts[i].messages[m].file.as_mut() else {
            return;
        };
        f.done += 1;
        if index == 0 {
            f.picture = crate::picture::is_image(data);
        }
        let (next, whole) = (f.done, f.complete());
        if whole || next % ACK_EVERY == 0 {
            let _ = self.transmit(&key, Body::FileAck { id, next });
        }
        if whole {
            self.got(i, Seen::File(id));
            self.happened.push(Happened::Alert {
                key,
                request: false,
            });
        }
    }

    fn write_part(&self, spool: &str, id: u32, index: u16, data: &[u8]) -> io::Result<()> {
        let record = seal_record(&self.key, &aad(id, index), data)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let path = self.spool_path(spool);
        let mut file = match index {
            0 => data::create_private(&path, false)?,
            _ => std::fs::OpenOptions::new().write(true).open(&path)?,
        };
        let at = (usize::from(index) * RECORD) as u64;
        file.set_len(at)?;
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&record)?;
        file.sync_data()
    }

    /// ADR-043: asks contact `i` for offered document `m`.
    pub fn download_file(&mut self, i: usize, m: usize) -> Result<(), String> {
        self.writable(i)?;
        let msgs = &self.data.contacts[i].messages;
        let f = msgs
            .get(m)
            .filter(|msg| !msg.mine)
            .and_then(|msg| msg.file.as_ref())
            .filter(|f| f.offered())
            .ok_or("nothing to download")?;
        let moving = |msg: &Msg| {
            !msg.mine
                && msg.file.as_ref().is_some_and(|f| {
                    !f.complete() && f.large.as_ref().is_some_and(|l| l.wanted && l.held)
                })
        };
        if msgs.iter().any(moving) {
            return Err("wait until their previous document arrives".into());
        }
        if self.data.spooled_theirs() + f.size as usize > MAX_SPOOLED_THEIRS {
            return Err("save or discard the documents you have first".into());
        }
        let id = f.id;
        let mut spool = [0; 12];
        getrandom::fill(&mut spool).map_err(|e| e.to_string())?;
        let key = self.data.contacts[i].key;
        if let Some(l) = self.data.contacts[i].messages[m]
            .file
            .as_mut()
            .and_then(|f| f.large.as_mut())
        {
            l.wanted = true;
            l.held = true;
            l.spool = spool.iter().map(|b| format!("{b:02x}")).collect();
        }
        if let Err(e) = self.transmit(&key, Body::FileWant { id }) {
            if let Some(l) = self.data.contacts[i].messages[m]
                .file
                .as_mut()
                .and_then(|f| f.large.as_mut())
            {
                (l.wanted, l.held) = (false, false);
            }
            self.persist();
            return Err(e);
        }
        Ok(())
    }

    /// Received document `m`, from the spool: whole and checked, or nothing.
    pub(super) fn large_bytes(&self, i: usize, m: usize) -> Option<Vec<u8>> {
        let f = self.data.contacts.get(i)?.messages.get(m)?.file.as_ref()?;
        let l = f.large.as_ref().filter(|l| l.held && f.complete())?;
        let mut out = Vec::with_capacity(f.size as usize);
        for index in 0..f.done {
            out.extend_from_slice(&self.read_part(&l.spool, f.id, index, f.size).ok()?);
        }
        (out.len() == f.size as usize).then_some(out)
    }

    /// Declines an offer, stops a transfer, or forgets a received document.
    pub(super) fn drop_large(&mut self, i: usize, m: usize) {
        let Some(f) = self.data.contacts[i].messages[m].file.as_mut() else {
            return;
        };
        match f.complete() {
            true => {
                f.large.iter_mut().for_each(|l| l.held = false);
                self.persist();
                self.sweep_spool();
            }
            false => self.stop_large(i, m, true, None),
        }
    }

    /// Transfers not finished in `OFFER_TTL`, answered or not, on both
    /// sides: a lost acknowledgement or a deleted chat must not hold a copy
    /// and block the next document for good. `true` if any.
    pub(super) fn expire_offers(&mut self) -> bool {
        let t = now();
        let mut changed = false;
        for m in self.data.contacts.iter_mut().flat_map(|c| &mut c.messages) {
            if stale(m, t)
                && let Some(l) = m.file.as_mut().and_then(|f| f.large.as_mut())
            {
                (l.stopped, l.held) = (true, false);
                changed = true;
            }
        }
        changed
    }
}

/// Ours still held, or theirs not whole, `OFFER_TTL` after the offer. A
/// received document that is whole waits to be saved, however old.
fn stale(m: &Msg, t: u64) -> bool {
    let Some(f) = m.file.as_ref() else {
        return false;
    };
    let Some(l) = f.large.as_ref().filter(|l| !l.stopped) else {
        return false;
    };
    let moving = match m.mine {
        true => l.held,
        false => !f.complete(),
    };
    moving && t >= m.at + OFFER_TTL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_transfers_expire_but_whole_documents_wait() {
        let doc = |mine: bool, done: u16, wanted: bool, held: bool| {
            let mut m = Msg::default();
            (m.mine, m.at) = (mine, 100);
            m.file = Some(FileMsg {
                id: 1,
                name: "a.pdf".into(),
                size: 2 << 20,
                done,
                picture: false,
                data: None,
                large: Some(Large {
                    held,
                    wanted,
                    ..Default::default()
                }),
            });
            m
        };
        let late = 100 + OFFER_TTL;
        // Asked for, then nothing more: both sides let go.
        assert!(stale(&doc(true, 10, true, true), late));
        assert!(stale(&doc(false, 10, true, true), late));
        // Never answered.
        assert!(stale(&doc(false, 0, false, false), late));
        // Not yet, already finished, or whole and waiting to be saved.
        assert!(!stale(&doc(true, 10, true, true), late - 1));
        assert!(!stale(&doc(true, 33, true, false), late));
        assert!(!stale(&doc(false, 33, true, true), late));
    }
}
