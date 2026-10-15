# termchat

Private text-only messenger for the terminal. End-to-end encrypted with [vodozemac](https://github.com/matrix-org/vodozemac) (Olm). The relay server only ever sees ciphertext.

**Status:** early design (milestone M0). Nothing works yet. Do not use for real communication.

## Acceptable use

termchat is for private conversations. It is **not** for trafficking people, weapons or
drugs, planning violence, or pushing anyone towards self-harm, and using it for that is
forbidden. We do not scan messages: that cannot work with end-to-end encryption and
would be a backdoor for everyone. Instead termchat gives such activity no reach: no user
search, no directory, no public channels or broadcasts, strangers land in "requests",
and you can block anyone (Ctrl+B). See [ADR-006](docs/adr/006-abuse-and-reports.md).

**What a relay operator can hand over:** public keys, who sent envelopes to whom and
when, and undelivered ciphertext. Not message contents, not nicknames, not IP history.

**Need help now?** https://findahelpline.com lists helplines in every country.

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
