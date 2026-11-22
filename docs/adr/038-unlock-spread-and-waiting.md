# ADR-038 — The unlock spread and a user who is waiting

Status — **accepted by the maintainer** (2026-11-22): B, C, and A as a setting, off by default. [Русская версия](038-unlock-spread-and-waiting.ru.md).

## Context

At unlock every link except the main identity's starts after a random 0–30 s (ADR-014, kept by ADR-023 for the link per contact). The relay then cannot tie a device's queues together by their connections coming up in the same second. The same random wait comes again before every reconnect.

The price shows in a live check on 2026-11-22: a contact wrote 7 s after the desktop app was unlocked, and the message appeared about 20 s later, while the status bar said "online" (the main identity's link). Nothing in the app explains the wait. A message sent in that window waits the same way, marked "sending". A wake-up (`Relay::reconnect`, a push through `Relay::check`) does not cut this first wait: it only cuts the backoff after a lost connection.

All three apps share the engine, so all three behave this way.

## What the spread protects

- **With Tor**, each link has its own circuit (stream isolation), so the relay sees no common IP. Connection timing is then the main way left to group a device's queues: the spread is what keeps them apart.
- **Without Tor**, every link comes from the same IP. ADR-014 already says the relay can link them that way. The spread adds little: some protection only where many people share an address (carrier NAT, a household).

## Options

A. **A link the user is waiting on skips the rest of its wait:** the chat or group the user opens, one they send to, and on Android the queue a push names. Only that link; the others keep their random delays. Cost under Tor: if the user opens a chat right after unlock, that contact's link comes up seconds after the main identity's, which is the correlation the spread hides. Sending adds no new signal: the relay sees the write in any case.

B. **No spread for direct links (Tor off).** The relay can group them by IP already (ADR-014). Fixes the wait for people without Tor at almost no cost. Tor is on by default on Android and in the desktop setup, so most people would keep the wait.

C. **Say why.** While a chat's link is still in its wait, the chat header says "connecting…" instead of nothing. No privacy cost; the wait stays.

D. **A shorter window**, say 0–10 s. Smaller wait for everyone, weaker timing protection under Tor in proportion.

## Decision

B and C, and A as a setting, **Connect the open chat at once**, off by default.

- B: a link spreads its start at unlock, and its wait before each reconnect, only with Tor on.
- C: the chat or group header says "connecting…" while its link has not reached the relay.
- A: with the setting on, the open chat's or group's link, and a link a push names, skips the rest of its wait (`Relay::hurry`). It only cuts a wait in progress, so nothing is kept to cut a later one.

## Not changed

The protocol, the relay and the wire format. Only when the client dials.
