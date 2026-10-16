# ADR-005: relay storage

Status: accepted, implemented in M5. Date: 2026-10-15. Русская версия: [005-relay-storage.ru.md](005-relay-storage.ru.md).

## Context

All relay state lived in memory. A restart lost:
- undelivered envelopes (offline delivery breaks);
- prekeys (no new sessions with an offline user until they reconnect);
- bans (a banned key can come back, ADR-006).

This has to go to disk without turning the relay into a metadata archive.

## Decision

### Engine: SQLite (`rusqlite`, `bundled` feature)
- One file, `termchat-relay.db` (`--db`), in WAL mode. The relay sets
  `umask 077` at start, so the database, its `-wal`/`-shm` files and the report
  log are created with mode `0600` (the WAL file is created after the database
  and would otherwise follow the umask).
- Schema version in `PRAGMA user_version`, migrations at start.
- Operators can inspect bans and the queue with plain `sqlite3`.
- Rejected: `redb`. Pure Rust with no C to compile, but less proven, and there is
  nothing to inspect it with.

### What is stored on disk

| Table | Fields | Lifetime |
|---|---|---|
| `accounts` | ed25519, created_at, last_seen, banned | deleted with its keys and queue after 90 days without a connection |
| `prekeys` | ed25519, key, signature, is_fallback | until handed out |
| `queue` | id, recipient, sender, envelope, created_at | until `Ack` or a TTL of 14 days |

### What is NOT stored on disk (memory only, lost on restart)
- **Who wrote to whom** (the graph behind the new-recipient limit and report
  checks). On disk it would be a social graph handed to anyone who seizes the
  server. Cost: after a restart the new-recipient limit starts over, and a report
  counts only for conversations after the restart.
- Report counters before a ban. Only the outcome, the ban, goes to disk.
- Rate limits, live connections, IP addresses.

A social graph does exist on disk, but only inside the queue: `sender` and
`recipient` of undelivered envelopes, for at most 14 days. Delivery needs it.
Delivered envelopes are deleted right after `Ack`.

### Quotas and cleanup
- Per recipient: 1000 envelopes and 10 MiB.
- Per relay: `--max-queue` (10 GiB by default). Past it, `QueueFull`.
- Every 10 minutes: delete envelopes older than 14 days and accounts unseen for
  90 days.
- After deleting, `PRAGMA incremental_vacuum`, so freed pages do not hold old
  ciphertext longer than needed. Leftovers on SSDs are possible (see ADR-003),
  but they are ciphertext only.

### Concurrency
One SQLite connection under the same global lock as the in-memory state. All
queries are short (by key or id). If a relay gets busy: a connection pool and
per-key locks.

### Dropping the old fallback key (ADR-002)
The client stores when it rotated the fallback key and calls
`forget_fallback_key` TTL + 1 day (15 days) later. That is as long as an envelope
can live in the queue, so a message to the old key still arrives in time.

## What a seized relay disk shows

- users' public keys, registration and last-seen times;
- public prekeys;
- undelivered ciphertext with sender, recipient and time, at most 14 days old;
- the list of banned keys.

No contents, nicknames, IP addresses or history of delivered messages. Recorded
in `docs/threat-model.md`.

## Consequences

- A new dependency, `rusqlite` (bundled SQLite is compiled from C). Licences MIT
  and public domain, accepted by `cargo deny`.
- The relay is no longer stateless: `docs/self-host.md` explains how to back up
  and delete `termchat-relay.db`.
- Tests: an envelope sent to an offline recipient is delivered after a relay
  restart; a ban survives a restart; expired envelopes and accounts are deleted.

## Answers to the open questions
- 90 days before an inactive account is deleted and a 10 GiB global limit are
  the defaults (`Limits`).
- Banned accounts are not deleted for inactivity.
