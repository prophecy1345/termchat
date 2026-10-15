# termchat

Private text-only messenger for the terminal. End-to-end encrypted with [vodozemac](https://github.com/matrix-org/vodozemac) (Olm). The relay server only ever sees ciphertext.

**Status:** early design (milestone M0). Nothing works yet. Do not use for real communication.

- What is and is not protected: [docs/threat-model.md](docs/threat-model.md)
- Wire protocol: [docs/protocol.md](docs/protocol.md)
- Plan and milestones: [PLAN.md](PLAN.md)

License: AGPL-3.0-or-later.

## Try it locally

```sh
cargo build --release
target/release/termchat-relay --host 127.0.0.1 --listen 127.0.0.1:8080 &
TERMCHAT_HOME=/tmp/alice target/release/termchat   # relay URL: ws://127.0.0.1:8080/
TERMCHAT_HOME=/tmp/bob   target/release/termchat   # in another terminal
```

Ctrl+O shows your invite, Ctrl+A adds a contact from theirs, `?` lists all keys.
Over the internet run the relay behind a TLS proxy (e.g. Caddy) with `--trust-proxy`
and use a `wss://` URL.
