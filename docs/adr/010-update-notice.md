# ADR-010: telling the user a newer version is out

Status: **accepted** (maintainer, 2026-10-17). Русская версия: [010-update-notice.ru.md](010-update-notice.ru.md).

## Context

Users learn about a new version only by checking GitHub. Security fixes reach
them late. The client should say "termchat X is out" in the "what's new"
window and explain how to update for the way it was installed.

The usual way, asking GitHub or a website for the latest version, is a new
connection to a third party. It shows that IP address uses termchat, at what
times, and bypasses Tor unless routed through it (threat model: metadata).

## Decision

### 1. The relay says its version in the handshake
The relay adds one HTTP header to its WebSocket upgrade response:
`x-termchat-version: 0.1.11` (its own crate version). The client reads it from
the response it already gets. No new connection, no new party, no new frame:

- the client sends nothing new, so the relay learns nothing new;
- the header travels inside TLS (or Tor), like everything else on that
  connection;
- an older client ignores the header; an older relay does not send it, and the
  client then says nothing.

New `ServerFrame` variants were rejected: an older client drops the connection
on a frame it cannot decode, so it would reconnect forever.

The public relay is redeployed with every release, so its version is the
latest. A self-hosted relay reports its own version, which is what the
operator runs; self-host.md says so.

### 2. What the client shows
If the relay's version is newer than the client's (numeric `major.minor.patch`
compare):

- the status bar says `termchat X is out: Ctrl+P, n`, and the menu item is
  marked, like unread news;
- the "what's new" window starts with "termchat X is out" and how to update,
  chosen from where the running binary lives:
  - under a Homebrew prefix (`/Cellar/`): `brew update && brew upgrade termchat`;
  - in `~/.local/bin`: the install.sh command from the README;
  - on Windows: the install.ps1 command;
  - anything else: the releases page.
- The notes of the new version are not shown: they are in the new binary. The
  window gives the releases link for them.

The client never downloads or runs anything itself.

### 3. What a malicious relay can do
Claim a higher version than exists: the user sees a notice, runs their package
manager, and nothing changes. The command and the links are built into the
client; nothing from the relay but the version number is shown, and it is
shown only if it parses as `digits.digits.digits`.

## Metadata, honestly

Nothing new: no extra connection, no extra request. The relay already knows
when and from where a client connects.
