# ADR-033 implementation review

## Send path

- The CLI and daemon resolve one selector and pin the resulting group ID. The
  engine checks the local reader grant, roster membership and device revocation
  before script queueing and broadcast. It correlates relay confirmation with
  the outgoing text and records when confirmation was handled.
- A transport frame carries the group session's write permission. A move or
  loss of access invalidates the permission, including for retries after a
  reconnect. The admin saves the newly rotated session while the move is
  still pending and saves the completed move before flushing texts. A
  temporary refusal to create the queue or readers schedules another attempt.
- The primary admin device authenticates moves and reader assignments.
  Reader grants from the old queue cannot identify linked devices on the new
  queue. Device changes received during the move update the pending grants
  before the admin shares the new key.

## Receive path

- Megolm authenticates the sender and group. The engine resolves the sender
  to its roster member and excludes copies from the local identity. The
  replay floor rejects frames from before the retained window. The engine
  stores an independently defaulted script marker for incoming texts and
  reads the current text and original reply ID at handoff.
- The engine saves the markers before forwarding a batch and restores them
  if the save fails. The daemon hands the batch only to readers registered
  for that group. Every reader has its own 64-record channel and terminal
  status. The CLI cleans values before terminal or JSON output and reports
  stdout failures.

## Verification

- The local relay scenario uses an admin, a CLI and daemon identity, and a
  second recipient. It covers stdin, JSON replies, old history, exact and
  ambiguous selectors, renaming, concurrent reads, direct-chat isolation,
  a disconnected group link, restart, late acceptance, permanent refusal,
  removal during a follow, and a broken stdout pipe.
- Engine and core tests cover save failure on enqueue, pre-send save failure,
  unrelated acknowledgements, deleted or expired texts, read-marker
  rollback, stale queue retries, queue moves, linked-device revocation,
  author binding, and replay. A daemon unit test fills one subscriber's
  buffer while a second subscriber continues.

The runtime checks used a local relay on macOS.

```sh
cargo test --offline -p termchat -p termchat-engine -p termchat-core --no-default-features
cargo clippy --offline -p termchat -p termchat-engine -p termchat-core --no-default-features --all-targets
cargo check --workspace --offline
```

An in-flight frame can reach the relay while the sender learns of a move or
revocation. A successful handoff can precede a failure writing to stdout.
