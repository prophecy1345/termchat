# Running your own relay

Русская версия: [self-host.ru.md](self-host.ru.md).

A relay forwards encrypted envelopes. It never sees message contents, nicknames
or contact lists. See [threat-model.md](threat-model.md) for what it does see.

You need a small Linux server with a domain name (for HTTPS) and, optionally,
Tor (to also offer an onion address).

## What the relay stores

All in its data folder, mode `0600` (ADR-005):

| File | Contents | Kept |
|---|---|---|
| `termchat-relay.db` (+ `-wal`, `-shm`) | public keys, prekeys, undelivered ciphertext with sender/recipient/time, bans | envelopes until delivered or 14 days; accounts until 90 days idle |
| `termchat-reports.log` | reports users chose to send you (ADR-006): time, reported key, disclosed text | until you delete it |

Nothing else: no IP addresses, no access log, no message history.

## Option A: Docker + Caddy (simplest)

```sh
git clone https://github.com/prophecy1345/termchat && cd termchat/deploy
# put your domain into Caddyfile and docker-compose.yml (relay.example.org)
docker compose up -d
```

Caddy gets a Let's Encrypt certificate and forwards WebSocket traffic to the
relay, which listens only inside the Docker network. The Caddyfile discards
access logs on purpose. Users enter `wss://relay.example.org/` on first run.

## Option B: bare binary + systemd

Download the release for your server (`termchat-…-x86_64-unknown-linux-musl.tar.gz`),
verify it (see the README), then:

```sh
sudo install -m 755 termchat-relay /usr/local/bin/
```

`/etc/systemd/system/termchat-relay.service`:

```ini
[Unit]
Description=termchat.app relay
After=network-online.target

[Service]
ExecStart=/usr/local/bin/termchat-relay --host relay.example.org --trust-proxy --listen 127.0.0.1:8080
DynamicUser=yes
StateDirectory=termchat-relay
WorkingDirectory=/var/lib/termchat-relay
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now termchat-relay
```

Put Caddy (or nginx) in front, terminating TLS and proxying to `127.0.0.1:8080`:

```
relay.example.org {
    reverse_proxy 127.0.0.1:8080
    log {
        output discard
    }
}
```

Only use `--trust-proxy` when a proxy that sets `X-Forwarded-For` is in front;
otherwise anyone could fake their IP and dodge the per-IP limits.

## Adding an onion address (Tor)

Install Tor from your distribution (`apt install tor`; the Debian/Ubuntu package
includes the proof-of-work defence). Add to `/etc/tor/torrc`:

```
HiddenServiceDir /var/lib/tor/termchat/
HiddenServicePort 80 127.0.0.1:8081
HiddenServicePoWDefensesEnabled 1
```

Homebrew's and some other `tor` builds lack the proof-of-work module (it needs
`--enable-gpl`); there, drop the last line or tor refuses to start.

```sh
sudo systemctl restart tor
sudo cat /var/lib/tor/termchat/hostname   # xxxx.onion
```

Then run the relay with both names and the onion port:

```
termchat-relay --host relay.example.org --host xxxx.onion --trust-proxy \
  --listen 127.0.0.1:8080 --onion-listen 127.0.0.1:8081
```

Users enter `ws://xxxx.onion/` as the onion URL and answer "y" to "Use Tor?".
Connections on the onion port share one budget (ADR-004), so per-IP limits do
not lock out Tor users.

## Running it well

- **Backups.** Losing the database loses undelivered messages and prekeys
  (clients republish keys on their next connection). Back up with
  `sqlite3 termchat-relay.db ".backup backup.db"`; the backup holds the same
  metadata, so protect it the same way.
- **Reports.** Read `termchat-reports.log`, act, then delete the reviewed
  lines. Three reports from different people the key really wrote to ban it
  automatically. Bans are in the `accounts` table (`banned = 1`).
- **Disk encryption.** Use full-disk encryption on the server: deleted rows can
  linger on SSDs until TRIM.
- **Upgrades.** Stop, replace the binary, start. The schema version is stored in
  the database (`PRAGMA user_version`) so later releases can migrate it; so far
  version 2 adds the UnifiedPush endpoint (ADR-012) and migrates on start. Clients compare their version with the one your
  relay reports and tell their users when it is newer (ADR-010), so keep the
  relay current: an old relay keeps quiet about new releases.
- **Shutting down for good.** `systemctl stop termchat-relay`, then delete the
  data folder. With Docker: `docker compose down -v`.
