# ADR-047: faster documents

Status: **accepted** (owner, 2026-11-25). Русская версия: [047-faster-documents.ru.md](047-faster-documents.ru.md). Changes the pace in [ADR-017](017-documents.md) and [ADR-043](043-large-documents.md) amendment 1. Nothing about encryption changes.

## Context

The owner asked for documents to go as fast as possible without losing security. Today:

| What | Chunk | Client pace | Relay budget per queue | Time |
|---|---|---|---|---|
| Up to 1 MiB (push, ADR-017) | 16 000 B, 16 KiB bucket | 10 at once, then 1 per 3 s | `sends_per_queue`: 30, then 1/s, shared with text | 1 MiB in about **3 minutes** |
| Above 1 MiB (offer, ADR-043) | 64 000 B, 64 KiB bucket | 10 at once, then 1 per 0.3 s | `parts_per_queue`: 30, then 4/s | 25 MiB in about **2 minutes** |

So a 900 KiB photo is slower than a 4 MiB one. Both paces are set by us, not by the network: the client waits, the relay rate-limits. The encryption (Olm per chunk) and the disk (sealed spool records) cost milliseconds per chunk.

What actually protects a queue from flooding is the queue quota (10 MiB, 1000 envelopes) and the relay-wide caps (ADR-030 §7); ADR-043 amendment 1 already said so. The rate only decides how fast a quota fills.

## Decision

### 1. The relay counts 16 KiB envelopes with the parts

- An envelope whose body is larger than the 4 KiB bucket's (`MEDIUM_BODY_BYTES`, the largest body the two smallest buckets produce) draws from `parts_per_queue`. Everything smaller draws from `sends_per_queue` as now.
- So chunks of small documents no longer share the 1/s budget with text. A long text (16 KiB bucket) also goes on the parts budget, which is only faster for it.

### 2. Parts budget: 10 a second

- `parts_per_queue`: **60 in a burst, then 10 a second** (was 30, then 4).
- `sends_per_queue` (text, receipts, everything small) unchanged: 30, then 1/s.
- Queue quota (10 MiB, 1000 envelopes), relay-wide caps and `frames_per_connection` (20/s) unchanged.

### 3. Client pace on a relay of this version or later

- Large documents: a part every **0.12 s**, burst 30 (was 0.3 s, burst 10).
- Small documents: a chunk every **0.12 s**, burst 30, from the same budget as parts (was 3 s, burst 10).
- On an older relay the client keeps today's pace, as ADR-043 amendment 1 does for relays before 0.30.0.
- The window stays at 64 parts (4 MiB) unacknowledged, ack every 16 parts: at 8 parts a second the window holds 8 s of sending, more than a round trip through the relay.

### 4. Expected times

- 1 MiB: **under 10 seconds** (was about 3 minutes).
- 25 MiB: **under a minute** (was about 2 minutes), if both links carry about 4.5 Mbit/s. Over Tor or a slow link the network is the limit, not us.

## Security and honest limits

- **No crypto change**: the same Olm sessions, chunk sizes, buckets, spool sealing and validation in `proto`. No new `Body`, no new protocol level.
- **Metadata unchanged**: the relay already sees bucket sizes and timing. A transfer now shows as a shorter, denser burst; the total bytes are the same.
- **Flooding a queue** someone knows: the 10 MiB quota fills in about 10 s instead of about 35 s. The quota, not the rate, is the limit, and it does not change. The receiver's text still has the remaining 6 MiB and its own 1/s budget.
- **Relay bandwidth**: one queue can take up to about 640 KB/s in (was about 260 KB/s). Our VPS has room for it.
- **Battery and data**: the same bytes, sent sooner. Large documents still go only after "Download" (ADR-043).

## Note (2026-11-25, audit finding 10)

The client keeps one frame in flight per queue and sends the next once the relay has taken it, so a refused part never lets a later one overtake it. The pace is therefore also bounded by the round trip to the relay: about 5 parts a second at 200 ms, so 25 MiB takes about 80 s there, and longer over Tor. The times above assume a relay less than about 100 ms away. Several frames in flight would need parts to be re-ordered after a refusal; not done.

## Rejected

- **Larger chunks or a new bucket** above 64 KiB: more padding wasted on the last chunk, a bigger frame limit on every relay, and a new size that stands out more. The gain is small next to the pace change.
- **Removing the rate for parts altogether**: the budget still keeps one client from taking a relay's bandwidth in a burst; 10/s is above what most links carry anyway.
- **Padding small-document chunks into the 64 KiB bucket** to reach the parts budget: four times the traffic for each chunk.
- **Blob storage on the relay**: the reasons in ADR-043 still hold (rule 1, rule 14).

## Rollout

The relay goes first (ADR-010: it is redeployed with the release). A client sees the relay's version and only then uses the faster pace, so an old relay never rate-limits a new client into errors.

## Dependencies

None new.
