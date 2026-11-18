# ADR-029: daemon mode for scripts

Status: **proposed** (2026-11-18). Русская версия: [029-daemon.ru.md](029-daemon.ru.md). Revisits decision 1 of [ADR-024](024-scriptable-mode.md).

## Context

In scriptable mode (ADR-024) every command unlocks the state file, connects to the relay, does its job and exits. That is right for an occasional notice, and slow for anything more:

- **A bot that answers.** "status" from the phone, an answer within a second. Over Tor each `send` costs a new connection, 10–30 seconds.
- **Frequent notices** from CI, cron and monitoring: each script needs the password file and connects on its own.
- **Several scripts on one identity:** the second gets "already open" and fails (ADR-024, section 5).

The maintainer asked for a daemon mode (2026-11-18).

## Decision

### 1. `termchat daemon`

```
termchat daemon [--password-file PATH | --password-stdin]
```

- Unlocks the identity once (the same password sources and duress behaviour as ADR-024, section 2), holds the state lock, keeps its relay links open and runs the engine's timers, as the TUI does.
- Listens on a **Unix socket inside the state folder**: `$TERMCHAT_HOME/daemon.sock`. The folder is already `0700`; the socket is made `0600`. A socket left over from a crashed daemon is removed at start, but only after checking that nothing answers on it.
- **Peer check:** each connection's user id (`peer_cred`, from the kernel) must be the daemon's own; anything else is closed without a word.
- No TCP, no HTTP, nothing on the network (ADR-024 rejected an open port, and that stays).
- Stops on SIGTERM or SIGINT: saves the state, removes the socket, exits 0.
- Unix only (Linux, macOS). On Windows `termchat daemon` says it is not supported.

### 2. The commands use it by themselves

`send`, `send-file`, `recv`, `contacts`, `invite` and `add` first look for a running daemon for the same `TERMCHAT_HOME`. If one answers, they go through it and need **no password**. If none answers, they work as today. `init` never goes through it.

So a script does not change: the same `termchat send owner "backup failed"` runs in about a millisecond when the daemon is up.

### 3. The socket protocol

One JSON object per line in each direction, the same shape as `recv --json` (ADR-024, section 4):

- request: `{"cmd": "send", "to": "...", "text": "..."}`, `{"cmd": "send_file", "to": "...", "name": "...", "data": "<base64>"}`, `{"cmd": "recv", "from": null, "wait": 30, "follow": false}`, `{"cmd": "contacts"}`, `{"cmd": "invite"}`, `{"cmd": "add", "invite": "..."}`;
- answer: `{"ok": true, ...}` or `{"ok": false, "code": 3, "error": "..."}`, with the exit codes of ADR-024, section 3; `recv` sends one line per message, then `{"ok": true}` unless it follows.

The file for `send-file` is read by the command, not by the daemon: the daemon opens no paths it is given. Limits are those of ADR-017 (1 MB a document), and a request line over 2 MB is refused.

Text from contacts is cleaned (`clean_text`) by the daemon before it goes out, as in ADR-024.

### 4. Several readers

- `recv` without `--follow` takes the unread messages and marks them read, as now.
- Every `recv --follow` connected at the time gets each new message; it is marked read once one of them has it.
- At most 16 connections at once; more are closed.

## Threat model

- **The password and the keys stay in memory** for as long as the daemon runs. Whoever gets root, or the daemon's user, reads and writes as this identity. That was already true of the password file next to the state (ADR-024), and of an open TUI. A daemon is for a server identity, never for the main one; the docs say so.
- **The socket is a door for every process of that user.** With the password file it was the same: any process of the user could read the file. The peer check keeps other users out; a sandbox needs a user of its own.
- **Nothing new for the relay:** the same queues; one long connection instead of many short ones, which hides less timing than before only in that sends are no longer preceded by a fresh connect.
- **No broadcasts, no accepting requests** (rule 15, ADR-024): the daemon sends to one contact per request and has no command to accept a contact.

## Not a new cryptographic protocol (rule 1)

The same engine, sessions and queues. Only how commands reach the engine changes. No new dependency: tokio's `UnixListener` and `peer_cred`, its `signal` feature for SIGTERM, and `serde_json` and `base64`, which the client already uses.

## Rejected

- **A TCP port on localhost:** any local user could connect; Unix sockets check the peer's user.
- **The socket in `/run` or `/tmp`:** shared folders; the state folder is private already.
- **The daemon reads files by path:** a confused-deputy risk; the command reads them as its own user.
- **A separate `termchat-daemon` binary:** one more artifact; a subcommand is enough.

## What changes in the code

- `client`: `daemon` (listener, peer check, request loop), and in `send`/`recv`/... a first try at the socket.
- Docs: a "Daemon" section in the scripting guide (EN/RU) with a systemd unit, the threat model line, and ADR-024 points to this ADR.
- `core`, `proto`, `relay` and `engine`'s protocol do not change.
