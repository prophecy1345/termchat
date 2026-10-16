# ADR-001: language, crypto core, serialization, licence

Status: accepted. Date: 2026-10-15. Русская версия: [001-stack.ru.md](001-stack.ru.md).

## Context

We need a private, text-only messenger with end-to-end encryption, offline
delivery through a public relay, and a core that can later be bound to Swift
and Kotlin. Home-made cryptography is not allowed.

## Decisions

### Rust 1.99, edition 2024
- Memory safety without a GC, one static binary for the relay, UniFFI for mobile
  platforms.
- Pinned in `rust-toolchain.toml`, `rust-version = "1.99"` in the workspace.

### vodozemac 0.11 for end-to-end encryption
- Matrix's Rust implementation of Olm (3DH + Double Ratchet) and Megolm, licensed
  Apache-2.0. Externally audited (Least Authority, 2022).
- Out of the box: one-time and fallback keys (a session can start while the
  recipient is offline), forward secrecy, post-compromise security, serializable
  state (`pickle`). Megolm will help with groups.
- Rejected:
  - `snow` (Noise): interactive handshakes only; offline delivery would have to
    be built from primitives;
  - `libsignal`: AGPL, not on crates.io, heavy and tied to Signal's server.
- Risk: the audit covers the 2022 version; newer versions were not audited as a
  whole. Accepted: still far better than any code of our own.

### postcard for serialization
- Compact, no `std` dependencies, stable format (1.x), MIT OR Apache-2.0.
- Rejected: `bincode`, RUSTSEC-2025-0141 "Bincode is unmaintained" (January
  2026); `cargo deny` would block it.
- Size limits are checked before decoding (the WS frame length), not after.

### AGPL-3.0-or-later
- Anyone who runs a modified relay as a service must publish the code. This
  protects the users of public relays.
- Dependencies must be AGPL-compatible: the allowed licences are listed in
  `deny.toml`.

### Strict lints across the workspace
- `unsafe_code = "forbid"` in every crate (stricter than the original plan, which
  asked it only for `core` and `proto`). If `unsafe` is ever needed in `relay` or
  `client`, that is a separate discussion.
- `clippy::unwrap_used` and `clippy::expect_used` are denied outside tests.

### Package names
`termchat-proto`, `termchat-core`, `termchat-relay`, `termchat`. A package cannot
be called `core`: it clashes with Rust's built-in crate. The folders in
`crates/` keep the short names.

## Consequences

- The cryptographic protocol is defined entirely by vodozemac. Our code is
  responsible for transport, storage and key-binding checks
  (`docs/protocol.md`, section 7).
- vodozemac updates are reviewed against the changelog: a change in the pickle
  format requires a storage migration.
