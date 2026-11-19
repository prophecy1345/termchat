# ADR-031 — Server liveness monitoring with `termchat watch`

Status — **accepted by the maintainer**. [Русская версия](031-watch.ru.md).

## Context

A server that goes down cannot send its own alert. A watcher on another server or laptop listens for heartbeats from the server identity and messages the owner when they stop. Both sides use existing termchat queues and the [daemon](029-daemon.md). The owner runs the watcher.

## Decision

### 1. Two processes using the daemon

```sh
# On the monitored server
termchat watch send WATCHER --every 300

# On the separate watcher host
termchat watch monitor SERVER --notify OWNER --after 900
```

Both commands run in the foreground and use the daemon's Unix socket for their `TERMCHAT_HOME` on Linux and macOS. A missing daemon or closed socket makes the command exit with an error. Use a service manager to keep the commands running. The sender tries the first heartbeat immediately and the rest at the configured interval. It skips intervals missed during a pause. Intervals take positive whole seconds. Set the watcher's threshold above the send interval with room for reconnects and delivery delays.

Use separate service identities for the server and watcher. The commands resolve contact names at startup and pin the keys while they run. `WATCHER`, `SERVER` and `OWNER` must be accepted, fingerprint-verified contacts of the respective identity. Deleting a pinned contact, blocking it or removing its verification makes the command exit with an error. The owner receives the alert as an ordinary direct message, which a client without `watch` can read.

### 2. Heartbeats inside Olm

Append `Body::Heartbeat { sent_at: u64 }` to the known body variants. `sent_at` is Unix time in seconds. Use the existing Olm session, padding and direct contact queue. Heartbeats travel between the primary devices of the service identities. The watcher accepts them only from the pinned server key through that contact's direct queue. Group messages, invitations, copies between one's own devices and messages from linked devices cannot update the timer.

Deliver the heartbeat through a dedicated event for `watch monitor`. The heartbeat itself never appears in chat history, unread messages, notifications or `recv` output. Ordinary message gap diagnostics and the rules for saving Olm state and acknowledging delivery still apply. Clients with `Body::Unknown` support skip the unfamiliar message kind. `Understands` stays at its current level because decoding support says nothing about whether a watcher is running. The relay and outer envelope format stay the same.

Hand a heartbeat to the transport only when the connection is ready. The daemon keeps at most one heartbeat per contact awaiting a relay answer, including across reconnects. Skip later intervals while the transport retries that envelope. Heartbeats do not enter the client's persistent outgoing queue. The relay can still store accepted ciphertexts until delivery or the queue's usual TTL.

### 3. Freshness and alerts

- At startup the watcher waits one full threshold. The countdown uses a monotonic clock.
- On receipt, the watcher checks `sent_at` against its wall clock. It discards a heartbeat at or beyond the age threshold, more than 60 seconds ahead of its clock, or no newer than the last accepted timestamp. For age calculations an allowed future timestamp is treated as the receipt time. An accepted heartbeat can extend the deadline only as far as its remaining freshness, `threshold − age`, from receipt. This check accounts for delayed queue deliveries.
- When the deadline passes, the watcher queues one ordinary message such as “box-1 has been silent for 15 minutes” and records the alert locally. The ordinary send result exposes a permanent relay refusal locally. A new, fresh heartbeat re-arms the alert. Restarting the watcher starts a new grace period and can produce another alert for an ongoing outage.

## Threat model and limits

The relay sees ciphertext timing and sizes in the existing queue. The regular rhythm can reveal when the server is active and when it appears to stop. Olm protects the timestamp inside the heartbeat and the text of the alert. A relay can cause an alert by delaying or dropping heartbeats. It can also prevent delivery of the alert itself. A heartbeat is evidence that the identity was recently reachable over this path. It does not check the health of an application on the server.

Freshness checks depend on synchronized clocks. A stopped or sleeping watcher cannot report an outage until it runs again. For timely alerts, use a separate, always-on watcher host and account for any shared relay dependency. Someone who controls the service identity's keys can send genuine heartbeats as that identity. Neither a heartbeat nor the owner's reply to an alert runs commands on the server.

## Alternatives considered

- **Ordinary text heartbeats.** They clutter chat history and `recv`. A dedicated body keeps heartbeats out of conversations and ties freshness to the encrypted send time.
- **A timer on the relay.** This simplifies setup, while revealing what the relay is being used to monitor and requiring the owner to trust it to issue alerts.
- **Counting from receipt.** A stale queued heartbeat could delay an alert for another full threshold.

## Acceptance criteria

With a local relay, the sender delivers heartbeats to the watcher and a third identity receives exactly one alert after the sender stops. After a fresh heartbeat, the next outage generates one new alert. Expired, duplicate and out-of-order heartbeats do not extend the deadline after reconnect. A heartbeat from a different key, a group or a linked device does not affect the timer. Ordinary `recv`, history and notifications stay free of heartbeats, and the alert arrives as ordinary text.
