# ADR-039: a webhook receiver for monitoring alerts

Status: **proposed** (2026-11-22), waiting for the maintainer. Русская версия: [039-alert-webhooks.ru.md](039-alert-webhooks.ru.md). Revisits "Rejected: a TCP port on localhost" of [ADR-029](029-daemon.md).

## Context

Teams already run monitoring: Prometheus with Alertmanager, Grafana, Uptime Kuma, Gatus. Their alerts go to Telegram bots, Slack or e-mail, so the text of every incident passes through a third party. termchat can already send from scripts (ADR-024), through a daemon (ADR-029), to a contact or a private group (ADR-033). What is missing is the door these tools knock on: they only speak HTTP webhooks. A shell script in between works, but each team writes its own and gets the quoting, retries and size limits wrong.

ADR-029 rejected a TCP port because any local user could connect to it. A webhook needs one, so this ADR has to answer that objection.

## Decision

### 1. `termchat hook`

```
termchat hook --listen 127.0.0.1:9099 --token-file /etc/termchat/hook-token \
    --route ops=group:Ops --route oncall=contact:alice
```

- A separate long-running subcommand next to the daemon. It holds **no password and no keys**: it parses requests and passes them to the daemon over its Unix socket (ADR-029). No daemon, no delivery: it answers `503`.
- `--listen` defaults to `127.0.0.1:9099`. Any other address must be given explicitly, and the command prints a warning that the token then crosses a network in the clear unless a TLS proxy sits in front. No TLS of its own: Caddy or nginx do that better, as for the relay.
- **Routes are fixed at start.** `POST /ops` goes to the private group `Ops`, `POST /oncall` to the contact `alice`. A request cannot name a recipient, so whoever holds the token can only write to these. Contacts must be accepted and verified, as in ADR-032; groups are ours (ADR-033). Unknown path: `404`.
- **Token:** every request carries `Authorization: Bearer <token>`. The token is read from a file (`0600`, at least 32 random bytes as hex or base64, `termchat hook --new-token` writes one), compared in constant time (`subtle`). Missing or wrong: `401`, nothing else said. This answers ADR-029's objection: another local user can connect but cannot send.

### 2. What becomes a message

Each route takes one of the formats, `--route ops=group:Ops:alertmanager` (default `auto`):

- `alertmanager`: Alertmanager's webhook JSON, which Grafana's unified alerting also sends. One message per request: `FIRING` or `RESOLVED`, then per alert its `alertname`, `instance` (if any) and the `summary` or `description` annotation.
- `kuma`: Uptime Kuma's JSON (`msg`).
- `text`: the body as plain text (Gatus and anything that can template a body).
- `auto`: Alertmanager if `alerts` is an array, Kuma if `msg` is a string, otherwise text.

Plain text only (rule 9): no markup, links stay text. Control characters are removed as in `clean_text`. At most 10 alerts per message, then "and N more"; at most 4,000 characters, cut at a line end. Labels other than the ones above are left out: they often hold internal hostnames and IPs that are not needed in a chat. A route can name more with `--labels job,severity`.

### 3. Answers and limits

- `200` once the daemon has handed the message to the relay (the relay accepted it), so Alertmanager's retries cover a relay outage; `503` if the daemon is down or the send failed; `400` for a body that is not the route's format; `413` over 1 MiB.
- At most 16 connections, a 10-second limit to read headers and body, HTTP/1.1 only, no keep-alive beyond one request.
- At most 30 messages a minute per route; above that the receiver answers `429` and Alertmanager retries later. Merging repeated alerts is ADR-040 / D3.3, not here.

### 4. Parser and dependencies

HTTP through `hyper` 1.x with `hyper-util` and `http-body-util`, already in the build through Arti: no new crate in `Cargo.lock`, only new direct dependencies of the client. JSON through `serde_json`, already used. No HTTP parser of our own. The JSON-to-text mapping gets negative tests and a fuzz target (rule 4).

## Threat model

- **The token is a send-only key** to the configured routes. A leak lets someone post text as the server identity to those chats, and nothing more: no reading, no other recipients, no contacts, no commands to the server. Rotate it by changing the file and restarting `hook`.
- **Alert text reaches the chat in full.** Whatever Alertmanager puts in annotations is in the encrypted chat, in history on every member's device. The docs say to keep secrets out of annotations.
- **Another local user** can connect to the port; without the token they get `401`. A local user who can read the token file can send. The file is `0600`, owned by the service user.
- **Non-loopback listen** exposes the token on the wire without a TLS proxy. Opt-in only, with a warning.
- **Nothing new for the relay:** the same daemon, queues and messages as `termchat send`.
- **Rule 15:** one route is one contact or one private group, set by the operator. No broadcast, no public group.

## Rejected

- **Reading from the daemon's socket directly:** Alertmanager, Grafana and Kuma cannot speak to a Unix socket.
- **The recipient in the URL or body:** a token holder could write to any contact.
- **TLS built in:** certificates and renewal are the reverse proxy's job; one more thing to get wrong.
- **A hand-written HTTP parser:** an attack surface for nothing, hyper is already here.

## What changes

- `client`: `hook` subcommand (listener, token check, routes, formats), docs section "Alerts from your monitoring" in the scripting guide (EN/RU) with Alertmanager, Grafana and Kuma examples and a systemd unit, threat model line.
- `core`, `proto`, `relay`, `engine`: no change.
