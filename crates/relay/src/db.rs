//! Relay storage on disk (ADR-005): accounts, prekeys, queue, bans.
//!
//! Deliberately absent: who wrote to whom (beyond undelivered envelopes),
//! report counts, IPs. Those stay in memory. Every call takes `now` (unix
//! seconds) so expiry is testable.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use termchat_proto::{Ed25519Key, Envelope, SignedKey};

pub type Result<T> = rusqlite::Result<T>;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    key BLOB PRIMARY KEY,
    created_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    banned INTEGER NOT NULL DEFAULT 0,
    fallback_key BLOB,
    fallback_sig BLOB,
    fallback_used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS prekeys (
    id INTEGER PRIMARY KEY,
    account BLOB NOT NULL REFERENCES accounts(key) ON DELETE CASCADE,
    key BLOB NOT NULL,
    sig BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS prekeys_account ON prekeys(account, id);
CREATE TABLE IF NOT EXISTS queue (
    id INTEGER PRIMARY KEY,
    recipient BLOB NOT NULL REFERENCES accounts(key) ON DELETE CASCADE,
    sender BLOB NOT NULL,
    envelope BLOB NOT NULL,
    size INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS queue_recipient ON queue(recipient, id);
CREATE INDEX IF NOT EXISTS queue_created ON queue(created_at);
";

pub struct Db {
    conn: Connection,
    /// Sum of queued envelope sizes, kept here instead of summing on every send.
    queue_bytes: u64,
}

pub struct Queued {
    pub id: u64,
    pub sender: Ed25519Key,
    pub envelope: Envelope,
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
        conn.execute_batch(SCHEMA)?;
        // Version 2: where to wake a key with UnifiedPush (ADR-012).
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 2 {
            conn.execute_batch(
                "ALTER TABLE accounts ADD COLUMN push TEXT; PRAGMA user_version = 2;",
            )?;
        }
        let queue_bytes: i64 =
            conn.query_row("SELECT COALESCE(SUM(size), 0) FROM queue", [], |r| r.get(0))?;
        Ok(Self {
            conn,
            queue_bytes: queue_bytes as u64,
        })
    }

    /// `None` for an unknown key, else whether it is banned.
    pub fn banned(&self, key: &Ed25519Key) -> Result<Option<bool>> {
        self.conn
            .query_row("SELECT banned FROM accounts WHERE key = ?", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn register(&self, key: &Ed25519Key, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO accounts (key, created_at, last_seen) VALUES (?1, ?2, ?2)
             ON CONFLICT(key) DO UPDATE SET last_seen = ?2",
            params![key, now],
        )?;
        Ok(())
    }

    pub fn ban(&self, key: &Ed25519Key) -> Result<()> {
        self.conn
            .execute("UPDATE accounts SET banned = 1 WHERE key = ?", [key])?;
        Ok(())
    }

    /// Adds one-time keys (keeping the newest `keep`) and replaces the fallback key.
    pub fn publish(
        &self,
        account: &Ed25519Key,
        one_time: &[SignedKey],
        fallback: Option<&SignedKey>,
        keep: usize,
    ) -> Result<()> {
        for k in one_time {
            self.conn.execute(
                "INSERT INTO prekeys (account, key, sig) VALUES (?, ?, ?)",
                params![account, k.key, k.signature.as_slice()],
            )?;
        }
        self.conn.execute(
            "DELETE FROM prekeys WHERE account = ?1 AND id NOT IN
             (SELECT id FROM prekeys WHERE account = ?1 ORDER BY id DESC LIMIT ?2)",
            params![account, keep as i64],
        )?;
        if let Some(k) = fallback {
            self.conn.execute(
                "UPDATE accounts SET fallback_key = ?, fallback_sig = ?, fallback_used = 0 WHERE key = ?",
                params![k.key, k.signature.as_slice(), account],
            )?;
        }
        Ok(())
    }

    /// One-time keys left and whether the fallback key was handed out.
    pub fn key_count(&self, account: &Ed25519Key) -> Result<(u32, bool)> {
        self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM prekeys WHERE account = ?1), fallback_used FROM accounts WHERE key = ?1",
            [account],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    /// Oldest one-time key (removed), or the fallback key if none or not allowed.
    pub fn claim(&self, target: &Ed25519Key, one_time_allowed: bool) -> Result<Option<SignedKey>> {
        if one_time_allowed {
            let row: Option<(i64, [u8; 32], Vec<u8>)> = self
                .conn
                .query_row(
                    "SELECT id, key, sig FROM prekeys WHERE account = ? ORDER BY id LIMIT 1",
                    [target],
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
                "SELECT fallback_key, fallback_sig FROM accounts WHERE key = ?",
                [target],
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
                "UPDATE accounts SET fallback_used = 1 WHERE key = ?",
                [target],
            )?;
        }
        Ok(fallback)
    }

    /// Envelopes and bytes queued for `recipient`, optionally only from `sender`.
    pub fn queue_stats(
        &self,
        recipient: &Ed25519Key,
        sender: Option<&Ed25519Key>,
    ) -> Result<(usize, u64)> {
        self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM queue WHERE recipient = ?1 AND (?2 IS NULL OR sender = ?2)",
            params![recipient, sender],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as u64)),
        )
    }

    /// Registration time, unix seconds.
    pub fn created_at(&self, key: &Ed25519Key) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT created_at FROM accounts WHERE key = ?",
                [key],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn total_queue_bytes(&self) -> u64 {
        self.queue_bytes
    }

    pub fn enqueue(
        &mut self,
        recipient: &Ed25519Key,
        sender: &Ed25519Key,
        envelope: &Envelope,
        now: i64,
    ) -> Result<u64> {
        let size = envelope.body.len();
        self.conn.execute(
            "INSERT INTO queue (recipient, sender, envelope, size, created_at) VALUES (?, ?, ?, ?, ?)",
            params![recipient, sender, envelope.encode(), size as i64, now],
        )?;
        self.queue_bytes += size as u64;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn queued(&self, recipient: &Ed25519Key) -> Result<Vec<Queued>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, sender, envelope FROM queue WHERE recipient = ? ORDER BY id")?;
        let rows = stmt.query_map([recipient], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, [u8; 32]>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, sender, bytes) = row?;
            // Written by us, so it decodes; skip rather than fail if the file was tampered with.
            if let Ok(envelope) = Envelope::decode(&bytes) {
                out.push(Queued {
                    id: id as u64,
                    sender,
                    envelope,
                });
            }
        }
        Ok(out)
    }

    /// Deletes a delivered envelope; only its recipient may do that.
    pub fn ack(&mut self, recipient: &Ed25519Key, id: u64) -> Result<()> {
        let size: Option<i64> = self
            .conn
            .query_row(
                "DELETE FROM queue WHERE id = ? AND recipient = ? RETURNING size",
                params![id as i64, recipient],
                |r| r.get(0),
            )
            .optional()?;
        self.queue_bytes = self.queue_bytes.saturating_sub(size.unwrap_or(0) as u64);
        Ok(())
    }

    /// Forgets a key with its prekeys and queue (ADR-009); bans are kept.
    pub fn unregister(&mut self, key: &Ed25519Key) -> Result<()> {
        let freed: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM queue WHERE recipient = ?",
            [key],
            |r| r.get(0),
        )?;
        let gone = self
            .conn
            .execute("DELETE FROM accounts WHERE key = ? AND banned = 0", [key])?;
        if gone > 0 {
            self.queue_bytes = self.queue_bytes.saturating_sub(freed as u64);
        }
        Ok(())
    }

    /// Stores or forgets the push endpoint; it goes with the account (ADR-012).
    pub fn set_push(&self, key: &Ed25519Key, endpoint: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET push = ? WHERE key = ?",
            params![endpoint, key],
        )?;
        Ok(())
    }

    pub fn push(&self, key: &Ed25519Key) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT push FROM accounts WHERE key = ?", [key], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    /// Drops envelopes older than `queue_ttl` and accounts unseen for `account_ttl` (seconds),
    /// then gives the freed pages back so old ciphertext does not linger in the file.
    pub fn expire(&mut self, now: i64, queue_ttl: i64, account_ttl: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM queue WHERE created_at < ?", [now - queue_ttl])?;
        self.conn.execute(
            "DELETE FROM accounts WHERE last_seen < ? AND banned = 0",
            [now - account_ttl],
        )?;
        self.conn.execute_batch("PRAGMA incremental_vacuum;")?;
        self.queue_bytes =
            self.conn
                .query_row("SELECT COALESCE(SUM(size), 0) FROM queue", [], |r| {
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

    fn env(to: u8) -> Envelope {
        Envelope {
            version: termchat_proto::VERSION,
            recipient: [to; 32],
            olm_type: 1,
            body: vec![7; 100],
        }
    }

    #[test]
    fn everything_but_memory_state_survives_reopen() {
        let path =
            std::env::temp_dir().join(format!("termchat-relay-test-{}.db", std::process::id()));
        {
            let mut db = Db::open(Some(&path)).unwrap();
            db.register(&[1; 32], 0).unwrap();
            db.register(&[2; 32], 0).unwrap();
            db.publish(&[2; 32], &[key(1), key(2)], Some(&key(9)), 100)
                .unwrap();
            db.enqueue(&[2; 32], &[1; 32], &env(2), 0).unwrap();
            db.ban(&[1; 32]).unwrap();
        }
        let mut db = Db::open(Some(&path)).unwrap();
        assert_eq!(db.banned(&[1; 32]).unwrap(), Some(true));
        assert_eq!(db.banned(&[3; 32]).unwrap(), None);
        assert_eq!(db.key_count(&[2; 32]).unwrap(), (2, false));
        assert_eq!(db.total_queue_bytes(), 100);
        let q = db.queued(&[2; 32]).unwrap();
        assert_eq!(
            (q.len(), q[0].sender, q[0].envelope.clone()),
            (1, [1; 32], env(2))
        );

        // Only the recipient can ack.
        db.ack(&[1; 32], q[0].id).unwrap();
        assert_eq!(db.queued(&[2; 32]).unwrap().len(), 1);
        db.ack(&[2; 32], q[0].id).unwrap();
        assert!(db.queued(&[2; 32]).unwrap().is_empty());
        assert_eq!(db.total_queue_bytes(), 0);
        drop(db);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    #[test]
    fn claims_oldest_first_then_fallback() {
        let db = Db::open(None).unwrap();
        db.register(&[2; 32], 0).unwrap();
        db.publish(&[2; 32], &[key(1), key(2), key(3)], Some(&key(9)), 2)
            .unwrap();
        // Trimmed to the newest two.
        assert_eq!(db.claim(&[2; 32], true).unwrap(), Some(key(2)));
        assert_eq!(db.claim(&[2; 32], false).unwrap(), Some(key(9)));
        assert_eq!(db.key_count(&[2; 32]).unwrap(), (1, true));
        assert_eq!(db.claim(&[2; 32], true).unwrap(), Some(key(3)));
        assert_eq!(db.claim(&[2; 32], true).unwrap(), Some(key(9)));
        db.publish(&[2; 32], &[], Some(&key(8)), 2).unwrap();
        assert_eq!(db.key_count(&[2; 32]).unwrap(), (0, false));
        assert_eq!(db.claim(&[5; 32], true).unwrap(), None);
    }

    #[test]
    fn old_envelopes_and_idle_accounts_expire_but_bans_stay() {
        let mut db = Db::open(None).unwrap();
        for k in [1, 2, 3] {
            db.register(&[k; 32], 0).unwrap();
        }
        db.ban(&[3; 32]).unwrap();
        db.enqueue(&[2; 32], &[1; 32], &env(2), 0).unwrap();
        db.enqueue(&[2; 32], &[1; 32], &env(2), 13 * DAY).unwrap();
        db.register(&[2; 32], 80 * DAY).unwrap();

        db.expire(15 * DAY, 14 * DAY, 90 * DAY).unwrap();
        assert_eq!(db.queued(&[2; 32]).unwrap().len(), 1);
        db.expire(91 * DAY, 14 * DAY, 90 * DAY).unwrap();
        assert_eq!(db.banned(&[1; 32]).unwrap(), None, "idle account removed");
        assert_eq!(
            db.banned(&[2; 32]).unwrap(),
            Some(false),
            "seen at day 80, kept"
        );
        assert_eq!(db.banned(&[3; 32]).unwrap(), Some(true), "bans are kept");
        assert_eq!(db.total_queue_bytes(), 0);
    }

    #[test]
    fn unregister_removes_keys_and_queue_but_not_bans() {
        let mut db = Db::open(None).unwrap();
        for k in [1, 2, 3] {
            db.register(&[k; 32], 0).unwrap();
        }
        db.publish(&[2; 32], &[key(1)], Some(&key(2)), 10).unwrap();
        db.enqueue(&[2; 32], &[1; 32], &env(2), 0).unwrap();
        db.enqueue(&[1; 32], &[2; 32], &env(2), 0).unwrap();
        let one = db.total_queue_bytes() / 2;
        db.unregister(&[2; 32]).unwrap();
        assert_eq!(db.banned(&[2; 32]).unwrap(), None);
        assert_eq!(db.claim(&[2; 32], true).unwrap(), None);
        assert!(db.queued(&[2; 32]).unwrap().is_empty());
        assert_eq!(db.queued(&[1; 32]).unwrap().len(), 1, "sent mail stays");
        assert_eq!(db.total_queue_bytes(), one);
        db.ban(&[3; 32]).unwrap();
        db.unregister(&[3; 32]).unwrap();
        assert_eq!(db.banned(&[3; 32]).unwrap(), Some(true));
    }

    #[test]
    fn push_endpoint_lives_with_the_account_and_old_files_migrate() {
        let path = std::env::temp_dir().join(format!("termchat-relay-v1-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            // A version 1 file, before the push column existed.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        }
        let mut db = Db::open(Some(&path)).unwrap();
        db.register(&[1; 32], 0).unwrap();
        assert_eq!(db.push(&[1; 32]).unwrap(), None);
        db.set_push(&[1; 32], Some("https://ntfy.sh/up1")).unwrap();
        assert_eq!(
            db.push(&[1; 32]).unwrap().as_deref(),
            Some("https://ntfy.sh/up1")
        );
        db.set_push(&[1; 32], None).unwrap();
        assert_eq!(db.push(&[1; 32]).unwrap(), None);
        db.set_push(&[1; 32], Some("https://ntfy.sh/up1")).unwrap();
        db.unregister(&[1; 32]).unwrap();
        assert_eq!(db.push(&[1; 32]).unwrap(), None);
        drop(db);
        // Opening again does not migrate twice.
        Db::open(Some(&path)).unwrap();
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }
}
