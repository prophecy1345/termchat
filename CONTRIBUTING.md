# Contributing to termchat

Thank you for helping. termchat protects people's conversations, so the rules
below are strict on purpose.

## Rules

1. **No home-made cryptography.** Cryptographic protocols come only from
   vodozemac. Combining primitives into a protocol of our own is not allowed.
   If something needs a primitive or protocol the chosen libraries do not have,
   open an issue first.
2. **Every protocol or security decision** is written up as an ADR in
   `docs/adr/` and waits for the maintainer's approval before any code.
3. **One step at a time.** Say what a change will do before starting it, and
   show what was checked when it is done.
4. **Before every commit:** `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. Crypto
   and parsers also need negative tests and fuzzing.
5. **No `unsafe`** (`unsafe_code = "forbid"` in every crate). If it is truly
   needed somewhere, explain why in an issue first.
6. **Secrets** never reach logs, errors or panic messages. Types holding key
   material implement `Zeroize`/`ZeroizeOnDrop` and never print their value in
   `Debug`.
7. **Few dependencies.** Justify every new one in the commit message and check it
   with `cargo deny` and `cargo vet`.
8. **No `unwrap()` or `expect()`** in code that handles outside input. Errors go
   through `Result` and `thiserror`.
9. **Text only.** No files, images, link previews or message formatting.
10. **Promise nothing impossible.** Never write "unbreakable"; describe the
    threat model honestly, metadata leaks included.
11. **Core without IO.** `core` and `proto` do not depend on tokio, the network
    or the file system; the caller reads and writes files.
12. Small commits with clear messages (Conventional Commits).
13. If a requirement is unclear or conflicts with the threat model, ask instead
    of guessing.
14. **No personal data.** No fields for real name, phone, email, birth date,
    location and the like. The nickname exists only inside end-to-end
    encryption. The relay stores nothing but public keys, prekeys and the queue
    of ciphertext.
15. **No reach and no scanning.** No user search, directories, public channels,
    broadcasts or public groups. Message contents are never scanned or filtered,
    neither in the client nor on the relay (ADR-006). Abuse is handled with
    invites, contact requests, limits, blocking and reports.

## Layout

- `crates/proto`: wire format (postcard), no IO.
- `crates/core`: sessions (vodozemac), encrypted store, no IO.
- `crates/relay`: the relay server (axum, SQLite).
- `crates/engine`: everything a client does besides drawing: transport, Tor,
  app data and its file, passwords (ADR-011).
- `crates/client`: the terminal client (ratatui), a shell over `engine`.
- `docs/`: threat model, protocol, self-hosting, ADRs.
- `site/`: the project website.
