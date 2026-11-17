//! Relay storage on disk (ADR-005, ADR-023): queues, their prekeys and
//! undelivered envelopes.
//!
//! Deliberately absent: users, keys of people, senders, IPs. A queue is two
//! random IDs and the SHA-256 of its owner's token. Every call takes `now`
//! (unix seconds) so expiry is testable.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use termchat_proto::{Envelope, QueueId, QueueToken, SignedKey};

pub type Result<T> = rusqlite::Result<T>;

/// Version 3: anonymous queues (ADR-023). Older files held accounts, keys and
/// who wrote to whom; they are erased, not migrated. Version 4 adds groups
/// (ADR-027) and keeps everything of version 3.
const VERSION: i64 = 4;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS groups (
    id BLOB PRIMARY KEY,
    send_id BLOB NOT NULL UNIQUE,
    token_hash BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS queues (
    recv_id BLOB PRIMARY KEY,
    send_id BLOB NOT NULL UNIQUE,
    token_hash BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    push TEXT,
    fallback_key BLOB,
    fallback_sig BLOB,
    fallback_used INTEGER NOT NULL DEFAULT 0,
    grp BLOB REFERENCES groups(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS queues_grp ON queues(grp);
CREATE TABLE IF NOT EXISTS prekeys (
    id INTEGER PRIMARY KEY,
    queue BLOB NOT NULL REFERENCES queues(recv_id) ON DELETE CASCADE,
    key BLOB NOT NULL,
    sig BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS prekeys_queue ON prekeys(queue, id);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    queue BLOB NOT NULL REFERENCES queues(recv_id) ON DELETE CASCADE,
    envelope BLOB NOT NULL,
    size INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_queue ON messages(queue, id);
CREATE INDEX IF NOT EXISTS messages_created ON messages(created_at);
";

pub struct Db {
    conn: Connection,
    /// Sum of queued envelope sizes, kept here instead of summing on every send.
    queue_bytes: u64,
    /// Last envelope id handed out. SQLite alone gives `max(id) + 1`, so an
    /// emptied queue starts again at 1, and a connection that already got
    /// ids up to N in its subscribe pages would skip the live ones (lib.rs).
    /// Kept here, ids only grow while the relay runs; a restart drops every
    /// connection, so starting over from the table then is safe.
    last_id: u64,
}

pub struct Queued {
    pub id: u64,
    pub envelope: Envelope,
}

pub fn token_hash(token: &QueueToken) -> [u8; 32] {
    Sha256::digest(token).into()
}

/// Constant time: how long a wrong token takes must not tell how close it was.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Db {
    /// `None` keeps everything in memory (tests).
    pub fn open(path: Option<&Path>) -> Result<Self> {
        // File permissions come from the umask set in `main` (0600 for the WAL too).
        let conn = match path {
            Some(p) => Connection::open(p)?,
            None => Connection::open_in_memory()?,
        };
        // auto_vacuum must be set before the first table exists.
        // synchronous = NORMAL: in WAL mode an fsync per checkpoint, not per
        // transaction, so a client cannot turn every frame into a disk flush
        // under the global lock (ADR-005). A power cut may lose the last
        // transactions; the file itself stays consistent.
        conn.execute_batch(
            "PRAGMA auto_vacuum = INCREMENTAL; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 3 {
            // ADR-023: nothing of the account era survives, and VACUUM
            // rewrites the file so its old pages are gone too.
            conn.execute_batch(&format!(
                "DROP TABLE IF EXISTS queue; DROP TABLE IF EXISTS prekeys; DROP TABLE IF EXISTS accounts;
                 VACUUM; PRAGMA user_version = {VERSION};"
            ))?;
        } else if version == 3 {
            // ADR-027: groups join; every queue stays.
            conn.execute_batch(&format!(
                "CREATE TABLE groups (id BLOB PRIMARY KEY, send_id BLOB NOT NULL UNIQUE,
                     token_hash BLOB NOT NULL, created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL);
                 ALTER TABLE queues ADD COLUMN grp BLOB REFERENCES groups(id) ON DELETE CASCADE;
                 PRAGMA user_version = {VERSION};"
            ))?;
        }
        conn.execute_batch(SCHEMA)?;
        let (queue_bytes, last_id): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(size), 0), COALESCE(MAX(id), 0) FROM messages",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(Self {
            conn,
            queue_bytes: queue_bytes as u64,
            last_id: last_id as u64,
        })
    }

    pub fn create(
        &self,
        recv_id: &QueueId,
        send_id: &QueueId,
        token: &QueueToken,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO queues (recv_id, send_id, token_hash, created_at, last_seen) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![recv_id, send_id, token_hash(token), now],
        )?;
        Ok(())
    }

    /// Whether `token` opens the queue. An unknown queue and a wrong token
    /// look the same to the caller.
    pub fn owns(&self, recv_id: &QueueId, token: &QueueToken) -> Result<bool> {
        let stored: Option<[u8; 32]> = self
            .conn
            .query_row(
                "SELECT token_hash FROM queues WHERE recv_id = ?",
                [recv_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(stored.is_some_and(|h| same(&h, &token_hash(token))))
    }

    /// The queue that `send_id` writes to.
    pub fn recv_id(&self, send_id: &QueueId) -> Result<Option<QueueId>> {
        self.conn
            .query_row(
                "SELECT recv_id FROM queues WHERE send_id = ?",
                [send_id],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn touch(&self, recv_id: &QueueId, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE queues SET last_seen = ? WHERE recv_id = ?",
            params![now, recv_id],
        )?;
        Ok(())
    }

    /// Forgets the queue with its prekeys and envelopes.
    pub fn delete(&mut self, recv_id: &QueueId) -> Result<()> {
        let freed: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM messages WHERE queue = ?",
            [recv_id],
            |r| r.get(0),
        )?;
        self.conn
            .execute("DELETE FROM queues WHERE recv_id = ?", [recv_id])?;
        self.queue_bytes = self.queue_bytes.saturating_sub(freed as u64);
        Ok(())
    }

    /// Adds one-time keys (keeping the newest `keep`) and replaces the fallback key.
    pub fn publish(
        &self,
        queue: &QueueId,
        one_time: &[SignedKey],
        fallback: Option<&SignedKey>,
        keep: usize,
    ) -> Result<()> {
        for k in one_time {
            self.conn.execute(
                "INSERT INTO prekeys (queue, key, sig) VALUES (?, ?, ?)",
                params![queue, k.key, k.signature.as_slice()],
            )?;
        }
        self.conn.execute(
            "DELETE FROM prekeys WHERE queue = ?1 AND id NOT IN
             (SELECT id FROM prekeys WHERE queue = ?1 ORDER BY id DESC LIMIT ?2)",
            params![queue, keep as i64],
        )?;
        if let Some(k) = fallback {
            self.conn.execute(
                "UPDATE queues SET fallback_key = ?, fallback_sig = ?, fallback_used = 0 WHERE recv_id = ?",
                params![k.key, k.signature.as_slice(), queue],
            )?;
        }
        Ok(())
    }

    /// One-time keys left and whether the fallback key was handed out.
    pub fn key_count(&self, queue: &QueueId) -> Result<(u32, bool)> {
        self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM prekeys WHERE queue = ?1), fallback_used FROM queues WHERE recv_id = ?1",
            [queue],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    /// Oldest one-time key (removed), or the fallback key if none or not allowed.
    /// A contact queue has no fallback key (ADR-023), so it answers `None` then.
    pub fn claim(&self, queue: &QueueId, one_time_allowed: bool) -> Result<Option<SignedKey>> {
        if one_time_allowed {
            let row: Option<(i64, [u8; 32], Vec<u8>)> = self
                .conn
                .query_row(
                    "SELECT id, key, sig FROM prekeys WHERE queue = ? ORDER BY id LIMIT 1",
                    [queue],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((id, key, sig)) = row {
                self.conn
                    .execute("DELETE FROM prekeys WHERE id = ?", [id])?;
                return Ok(signed(key, &sig));
            }
        }
        let fallback = self
            .conn
            .query_row(
                "SELECT fallback_key, fallback_sig FROM queues WHERE recv_id = ?",
                [queue],
                |r| {
                    let key: Option<[u8; 32]> = r.get(0)?;
                    let sig: Option<Vec<u8>> = r.get(1)?;
                    Ok(key.zip(sig).and_then(|(k, s)| signed(k, &s)))
                },
            )
            .optional()?
            .flatten();
        if fallback.is_some() {
            self.conn.execute(
                "UPDATE queues SET fallback_used = 1 WHERE recv_id = ?",
                [queue],
            )?;
        }
        Ok(fallback)
    }

    /// Envelopes and bytes waiting in the queue.
    pub fn queue_stats(&self, queue: &QueueId) -> Result<(usize, u64)> {
        self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM messages WHERE queue = ?",
            [queue],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as u64)),
        )
    }

    pub fn total_queue_bytes(&self) -> u64 {
        self.queue_bytes
    }

    pub fn enqueue(&mut self, queue: &QueueId, envelope: &Envelope, now: i64) -> Result<u64> {
        let size = envelope.body.len();
        let id = self.last_id + 1;
        self.conn.execute(
            "INSERT INTO messages (id, queue, envelope, size, created_at) VALUES (?, ?, ?, ?, ?)",
            params![id as i64, queue, envelope.encode(), size as i64, now],
        )?;
        self.last_id = id;
        self.queue_bytes += size as u64;
        Ok(id)
    }

    /// The whole queue; the relay itself reads it in pages (`queued_after`).
    pub fn queued(&self, queue: &QueueId) -> Result<Vec<Queued>> {
        Ok(self.queued_after(queue, 0, usize::MAX)?.0)
    }

    /// Up to `limit` envelopes with an id above `after`, oldest first, and the
    /// id to continue from. A whole queue is 10 MiB: a connection holds one
    /// page of it at a time, not all of it.
    pub fn queued_after(
        &self,
        queue: &QueueId,
        after: u64,
        limit: usize,
    ) -> Result<(Vec<Queued>, u64)> {
        let mut stmt = self.conn.prepare(
            "SELECT id, envelope FROM messages WHERE queue = ? AND id > ? ORDER BY id LIMIT ?",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![queue, after as i64, limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        let mut last = after;
        for row in rows {
            let (id, bytes) = row?;
            last = id as u64;
            // Written by us, so it decodes; skip rather than fail if the file was tampered with.
            if let Ok(envelope) = Envelope::decode(&bytes) {
                out.push(Queued {
                    id: id as u64,
                    envelope,
                });
            }
        }
        Ok((out, last))
    }

    /// Deletes a delivered envelope, only from the queue it was in.
    pub fn ack(&mut self, queue: &QueueId, id: u64) -> Result<()> {
        let size: Option<i64> = self
            .conn
            .query_row(
                "DELETE FROM messages WHERE id = ? AND queue = ? RETURNING size",
                params![id as i64, queue],
                |r| r.get(0),
            )
            .optional()?;
        self.queue_bytes = self.queue_bytes.saturating_sub(size.unwrap_or(0) as u64);
        Ok(())
    }

    /// Stores or forgets the push endpoint; it goes with the queue (ADR-012).
    pub fn set_push(&self, queue: &QueueId, endpoint: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE queues SET push = ? WHERE recv_id = ?",
            params![endpoint, queue],
        )?;
        Ok(())
    }

    pub fn push(&self, queue: &QueueId) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT push FROM queues WHERE recv_id = ?", [queue], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    /// A group queue (ADR-027): what is sent to `send_id` goes to each of its readers.
    pub fn create_group(
        &self,
        id: &QueueId,
        send_id: &QueueId,
        token: &QueueToken,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO groups (id, send_id, token_hash, created_at, last_seen) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, send_id, token_hash(token), now],
        )?;
        Ok(())
    }

    /// Whether `token` is the admin's for group `id`; an unknown group looks the same.
    pub fn owns_group(&self, id: &QueueId, token: &QueueToken) -> Result<bool> {
        let stored: Option<[u8; 32]> = self
            .conn
            .query_row("SELECT token_hash FROM groups WHERE id = ?", [id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(stored.is_some_and(|h| same(&h, &token_hash(token))))
    }

    /// The group that `send_id` writes to, touched as used.
    pub fn group_of(&self, send_id: &QueueId, now: i64) -> Result<Option<QueueId>> {
        let id: Option<QueueId> = self
            .conn
            .query_row("SELECT id FROM groups WHERE send_id = ?", [send_id], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(id) = &id {
            self.conn.execute(
                "UPDATE groups SET last_seen = ? WHERE id = ?",
                params![now, id],
            )?;
        }
        Ok(id)
    }

    /// A reader of group `group`: an ordinary queue of its own.
    pub fn add_reader(
        &self,
        group: &QueueId,
        recv_id: &QueueId,
        send_id: &QueueId,
        token: &QueueToken,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO queues (recv_id, send_id, token_hash, created_at, last_seen, grp) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![recv_id, send_id, token_hash(token), now, group],
        )?;
        Ok(())
    }

    pub fn readers(&self, group: &QueueId) -> Result<Vec<QueueId>> {
        self.conn
            .prepare("SELECT recv_id FROM queues WHERE grp = ?")?
            .query_map([group], |r| r.get(0))?
            .collect()
    }

    /// Whether `recv_id` is a reader of `group`.
    pub fn is_reader(&self, group: &QueueId, recv_id: &QueueId) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT 1 FROM queues WHERE recv_id = ? AND grp = ?",
                params![recv_id, group],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
    }

    /// Forgets the group and every reader with what waits for them.
    pub fn delete_group(&mut self, id: &QueueId) -> Result<()> {
        let freed: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(m.size), 0) FROM messages m JOIN queues q ON m.queue = q.recv_id WHERE q.grp = ?",
            [id],
            |r| r.get(0),
        )?;
        self.conn.execute("DELETE FROM groups WHERE id = ?", [id])?;
        self.queue_bytes = self.queue_bytes.saturating_sub(freed as u64);
        Ok(())
    }

    /// Drops envelopes older than `queue_ttl` and queues unseen for `idle_ttl` (seconds),
    /// then gives the freed pages back so old ciphertext does not linger in the file.
    pub fn expire(&mut self, now: i64, queue_ttl: i64, idle_ttl: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM messages WHERE created_at < ?",
            [now - queue_ttl],
        )?;
        self.conn
            .execute("DELETE FROM queues WHERE last_seen < ?", [now - idle_ttl])?;
        // A quiet group lives on while any reader still comes by.
        self.conn.execute(
            "DELETE FROM groups WHERE last_seen < ?1 AND NOT EXISTS
                 (SELECT 1 FROM queues WHERE grp = groups.id AND last_seen >= ?1)",
            [now - idle_ttl],
        )?;
        self.conn.execute_batch("PRAGMA incremental_vacuum;")?;
        self.queue_bytes =
            self.conn
                .query_row("SELECT COALESCE(SUM(size), 0) FROM messages", [], |r| {
                    r.get::<_, i64>(0)
                })? as u64;
        Ok(())
    }
}

fn signed(key: [u8; 32], sig: &[u8]) -> Option<SignedKey> {
    Some(SignedKey {
        key,
        signature: sig.try_into().ok()?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const DAY: i64 = 86_400;

    fn key(n: u8) -> SignedKey {
        SignedKey {
            key: [n; 32],
            signature: [n; 64],
        }
    }

    fn env() -> Envelope {
        Envelope {
            version: termchat_proto::VERSION,
            olm_type: 1,
            body: vec![7; 100],
        }
    }

    /// Queue `n`: recv_id `[n]`, send_id `[n + 100]`, token `[n]`.
    fn queue(db: &Db, n: u8, now: i64) {
        db.create(&[n; 16], &[n + 100; 16], &[n; 32], now).unwrap();
    }

    fn remove(path: &Path) {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    #[test]
    fn everything_survives_reopen_and_only_the_hash_is_stored() {
        let path = std::env::temp_dir().join(format!("termchat-relay-q-{}.db", std::process::id()));
        remove(&path);
        {
            let mut db = Db::open(Some(&path)).unwrap();
            queue(&db, 1, 0);
            db.publish(&[1; 16], &[key(1), key(2)], Some(&key(9)), 100)
                .unwrap();
            db.enqueue(&[1; 16], &env(), 0).unwrap();
        }
        let mut db = Db::open(Some(&path)).unwrap();
        assert!(db.owns(&[1; 16], &[1; 32]).unwrap());
        assert!(!db.owns(&[1; 16], &[2; 32]).unwrap(), "wrong token");
        assert!(!db.owns(&[2; 16], &[1; 32]).unwrap(), "unknown queue");
        assert_eq!(db.recv_id(&[101; 16]).unwrap(), Some([1; 16]));
        assert_eq!(
            db.recv_id(&[1; 16]).unwrap(),
            None,
            "recv_id is not a send_id"
        );
        assert_eq!(db.key_count(&[1; 16]).unwrap(), (2, false));
        assert_eq!(db.total_queue_bytes(), 100);
        let stored: Vec<u8> = db
            .conn
            .query_row("SELECT token_hash FROM queues", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, token_hash(&[1; 32]));
        assert_ne!(stored, [1; 32]);

        // Only the queue an envelope is in can ack it.
        let q = db.queued(&[1; 16]).unwrap();
        assert_eq!((q.len(), q[0].envelope.clone()), (1, env()));
        db.ack(&[2; 16], q[0].id).unwrap();
        assert_eq!(db.queued(&[1; 16]).unwrap().len(), 1);
        db.ack(&[1; 16], q[0].id).unwrap();
        assert!(db.queued(&[1; 16]).unwrap().is_empty());
        assert_eq!(db.total_queue_bytes(), 0);
        drop(db);
        remove(&path);
    }

    #[test]
    fn an_account_era_file_is_erased() {
        let path = std::env::temp_dir().join(format!("termchat-relay-v2-{}", std::process::id()));
        remove(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE accounts (key BLOB PRIMARY KEY, push TEXT);
                 CREATE TABLE queue (id INTEGER PRIMARY KEY, recipient BLOB, sender BLOB);
                 INSERT INTO accounts VALUES (x'0101', 'https://ntfy.sh/secret');
                 INSERT INTO queue VALUES (1, x'0101', x'0202');
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }
        let db = Db::open(Some(&path)).unwrap();
        let tables: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(tables, ["groups", "messages", "prekeys", "queues"]);
        drop(db);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.windows(6).any(|w| w == b"secret"),
            "old rows are gone from the file, not just unlinked"
        );
        // Opening again keeps what the new schema holds.
        let db = Db::open(Some(&path)).unwrap();
        queue(&db, 1, 0);
        drop(db);
        let db = Db::open(Some(&path)).unwrap();
        assert!(db.owns(&[1; 16], &[1; 32]).unwrap());
        drop(db);
        remove(&path);
    }

    #[test]
    fn the_queue_reads_in_pages() {
        let mut db = Db::open(None).unwrap();
        queue(&db, 2, 0);
        queue(&db, 3, 0);
        for _ in 0..5 {
            db.enqueue(&[2; 16], &env(), 0).unwrap();
            db.enqueue(&[3; 16], &env(), 0).unwrap();
        }
        let (mut seen, mut after) = (Vec::new(), 0);
        loop {
            let (page, last) = db.queued_after(&[2; 16], after, 2).unwrap();
            assert!(page.len() <= 2);
            if page.is_empty() {
                break;
            }
            assert_eq!(last, page[page.len() - 1].id);
            seen.extend(page.iter().map(|q| q.id));
            after = last;
        }
        let all: Vec<u64> = db.queued(&[2; 16]).unwrap().iter().map(|q| q.id).collect();
        assert_eq!(
            (seen.len(), &seen),
            (5, &all),
            "every envelope once, in order"
        );
        assert_eq!(db.queue_stats(&[2; 16]).unwrap(), (5, 500));
    }

    #[test]
    fn claims_oldest_first_then_fallback_and_contact_queues_have_none() {
        let db = Db::open(None).unwrap();
        queue(&db, 2, 0);
        db.publish(&[2; 16], &[key(1), key(2), key(3)], Some(&key(9)), 2)
            .unwrap();
        // Trimmed to the newest two.
        assert_eq!(db.claim(&[2; 16], true).unwrap(), Some(key(2)));
        assert_eq!(db.claim(&[2; 16], false).unwrap(), Some(key(9)));
        assert_eq!(db.key_count(&[2; 16]).unwrap(), (1, true));
        assert_eq!(db.claim(&[2; 16], true).unwrap(), Some(key(3)));
        assert_eq!(db.claim(&[2; 16], true).unwrap(), Some(key(9)));
        db.publish(&[2; 16], &[], Some(&key(8)), 2).unwrap();
        assert_eq!(db.key_count(&[2; 16]).unwrap(), (0, false));
        assert_eq!(db.claim(&[5; 16], true).unwrap(), None);

        queue(&db, 3, 0);
        db.publish(&[3; 16], &[key(4)], None, 2).unwrap();
        assert_eq!(db.claim(&[3; 16], true).unwrap(), Some(key(4)));
        assert_eq!(db.claim(&[3; 16], true).unwrap(), None);
    }

    #[test]
    fn old_envelopes_and_idle_queues_expire() {
        let mut db = Db::open(None).unwrap();
        queue(&db, 1, 0);
        queue(&db, 2, 0);
        db.enqueue(&[2; 16], &env(), 0).unwrap();
        db.enqueue(&[2; 16], &env(), 13 * DAY).unwrap();
        db.touch(&[2; 16], 80 * DAY).unwrap();

        db.expire(15 * DAY, 14 * DAY, 90 * DAY).unwrap();
        assert_eq!(db.queued(&[2; 16]).unwrap().len(), 1);
        assert_eq!(db.total_queue_bytes(), 100);
        db.expire(91 * DAY, 14 * DAY, 90 * DAY).unwrap();
        assert!(!db.owns(&[1; 16], &[1; 32]).unwrap(), "idle queue removed");
        assert!(db.owns(&[2; 16], &[2; 32]).unwrap(), "seen at day 80, kept");
        assert_eq!(db.total_queue_bytes(), 0);
    }

    #[test]
    fn delete_removes_keys_envelopes_and_push() {
        let mut db = Db::open(None).unwrap();
        queue(&db, 1, 0);
        queue(&db, 2, 0);
        db.publish(&[2; 16], &[key(1)], Some(&key(2)), 10).unwrap();
        db.set_push(&[2; 16], Some("https://ntfy.sh/up1")).unwrap();
        assert_eq!(
            db.push(&[2; 16]).unwrap().as_deref(),
            Some("https://ntfy.sh/up1")
        );
        db.enqueue(&[2; 16], &env(), 0).unwrap();
        db.enqueue(&[1; 16], &env(), 0).unwrap();
        db.delete(&[2; 16]).unwrap();
        assert!(!db.owns(&[2; 16], &[2; 32]).unwrap());
        assert_eq!(db.recv_id(&[102; 16]).unwrap(), None);
        assert_eq!(db.claim(&[2; 16], true).unwrap(), None);
        assert_eq!(db.push(&[2; 16]).unwrap(), None);
        assert!(db.queued(&[2; 16]).unwrap().is_empty());
        assert_eq!(db.queued(&[1; 16]).unwrap().len(), 1, "other queues stay");
        assert_eq!(db.total_queue_bytes(), 100);
        db.set_push(&[1; 16], Some("https://ntfy.sh/up1")).unwrap();
        db.set_push(&[1; 16], None).unwrap();
        assert_eq!(db.push(&[1; 16]).unwrap(), None);
    }

    #[test]
    fn a_version_3_file_keeps_its_queues_and_gains_groups() {
        let path = std::env::temp_dir().join(format!("termchat-relay-v3-{}", std::process::id()));
        remove(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE queues (recv_id BLOB PRIMARY KEY, send_id BLOB NOT NULL UNIQUE,
                     token_hash BLOB NOT NULL, created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                     push TEXT, fallback_key BLOB, fallback_sig BLOB,
                     fallback_used INTEGER NOT NULL DEFAULT 0);
                 PRAGMA user_version = 3;",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO queues (recv_id, send_id, token_hash, created_at, last_seen) VALUES (?1, ?2, ?3, 0, 0)",
                params![[1u8; 16], [101u8; 16], token_hash(&[1; 32])],
            )
            .unwrap();
        }
        let db = Db::open(Some(&path)).unwrap();
        assert!(db.owns(&[1; 16], &[1; 32]).unwrap(), "the queue survives");
        db.create_group(&[7; 16], &[8; 16], &[7; 32], 0).unwrap();
        db.add_reader(&[7; 16], &[9; 16], &[10; 16], &[9; 32], 0)
            .unwrap();
        assert_eq!(db.readers(&[7; 16]).unwrap(), [[9; 16]]);
        drop(db);
        remove(&path);
    }

    #[test]
    fn a_group_goes_with_its_readers_and_lives_while_one_comes_by() {
        let mut db = Db::open(None).unwrap();
        let day = 86_400;
        db.create_group(&[7; 16], &[8; 16], &[7; 32], 0).unwrap();
        db.add_reader(&[7; 16], &[9; 16], &[10; 16], &[9; 32], 0)
            .unwrap();
        db.enqueue(&[9; 16], &env(), 0).unwrap();
        assert!(db.total_queue_bytes() > 0);
        // No message for 91 days, but its reader came by yesterday.
        db.touch(&[9; 16], 90 * day).unwrap();
        db.expire(91 * day, 14 * day, 90 * day).unwrap();
        assert!(db.owns_group(&[7; 16], &[7; 32]).unwrap());
        assert!(
            !db.owns_group(&[7; 16], &[9; 32]).unwrap(),
            "a reader's token is not the admin's"
        );
        db.delete_group(&[7; 16]).unwrap();
        assert!(db.readers(&[7; 16]).unwrap().is_empty());
        assert!(
            !db.owns(&[9; 16], &[9; 32]).unwrap(),
            "readers go with the group"
        );
        assert_eq!(db.total_queue_bytes(), 0);
    }
}
