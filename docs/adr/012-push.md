# ADR-012: waking the Android app for new messages

Status: **accepted** (maintainer, 2026-10-17; section 2 amended the same day, on 2026-11-11 and twice on 2026-11-14). Русская версия: [012-push.ru.md](012-push.ru.md).

## Context

A phone kills idle apps. To get messages while termchat is closed, either the
app keeps a connection open all the time, or something wakes it when a message
arrives. Google's push (FCM) is out: it needs Play Services and gives Google
the timing of every message. The owner chose UnifiedPush, with a foreground
service as the fallback.

## Decision

### 1. Foreground service (always available)
- A service keeps the relay connection open while the app is unlocked or
  "stay connected" is on.
- It shows the permanent notification Android requires: "termchat is
  connected", with no counts and no names.
- It costs battery. The relay protocol is unchanged.

### 2. UnifiedPush (if a distributor is installed)
UnifiedPush lets the user pick a push server, for example ntfy (self-hosted or
ntfy.sh). The app gets an endpoint URL from it.

- **New frame `ClientFrame::SetPush { endpoint: Option<String> }`**, only on
  an authenticated connection.
  - `Some` stores the endpoint for this key; `None` removes it.
  - Answer: `ServerFrame::PushSet`.
  - The new variants go at the end of the enums. An older relay answers
    `Malformed` and closes, so the client sends `SetPush` only when the
    relay's version header (ADR-010) is at least the release that adds it.
- **The relay** keeps the endpoint in a new `push` column of `accounts`. It
  goes with the account (`Unregister`, expiry).
- **When the relay queues a message for a key that is not online**, it POSTs
  an **empty body** to the endpoint:
  - at most once per key per 30 seconds;
  - with a 5-second timeout;
  - it does not retry.
- **The app**, woken by the push, only shows a "New message" notification
  (ADR-008). It does not connect: a locked or closed app has no keys in
  memory, and signing in to the relay needs them. The messages arrive when
  the user opens and unlocks it. (Amended 2026-10-17; the first version had
  the app connect and fetch, which the password makes impossible.)
- **An unlocked app** still holds its relay link, but in the background
  Android freezes it: the socket may be dead while the link waits up to 90 s
  of silence, or sleeps in its reconnect backoff (up to 60 s, on a clock that
  stops while the phone sleeps). So a wake-up, and the app coming on screen,
  ask the link to reconnect now: a backoff wait ends at once, and a link that
  heard nothing from the relay for over 35 s (wall clock) is dropped and
  dialled again. Unconfirmed sends are resent as after any reconnect. The relay
  sees the connection a moment after its push, the same timing as a user
  opening the app. (Amended 2026-11-11: messages came 30-90 s after the
  notification.)
- **A push always redials** (amended 2026-11-14). The relay pushes only
  when it holds no connection of that identity, so a push proves the link
  the app holds is dead, however recent its last ping. The relay can close
  a connection a few seconds after its last ping; if the close is lost
  (sleep, NAT, a network change), the app's socket stays open and silent,
  passes the 35-second check, and the message waits on the relay until the
  next send or push. So a push drops the link of the identity it was
  addressed to (its UnifiedPush instance) and dials again, unless a new
  connection came up since. The other identities stay as they are
  (ADR-014). The 35-second check remains for the app coming on screen.
- **The sleeping link** (amended 2026-11-14, replaces the rule above).
  In deep sleep Android stops the processor: the phone's kernel still takes
  in and acknowledges the relay's pings, but the app answers none of them.
  The relay closed such a connection after 60 s of silence, so the next
  message needed a push and a new connection. Under Tor that is a new onion
  circuit, 10-15 s; measured on a Pixel 9: deep sleep at 14:43:52, closed at
  14:45:02. Now:
  - The relay holds a silent connection for up to an hour, not 60 s, and
    keeps pinging it. Messages go into it as for any connection; they leave
    the queue only when the client acknowledges them, as before (ADR-007).
  - A connection that did not answer a ping by the next one (30 s later)
    counts as asleep. A message for it goes into the connection **and** sends a push,
    with the same 30-second gap as before. A connection that answers gets no
    push, and neither does a client without an endpoint.
  - The app, woken by the push, finds the message already in its socket and
    reads it: no new connection. To be sure the link lives, it sends a ping;
    if nothing comes from the relay within 5 s, it drops the link and dials
    again, as before. Coming on screen does the same after 35 s of silence,
    instead of dropping the link outright.
  - A new connection of the same identity still replaces the old one, so the
    relay holds at most one per identity; the limits on connections per IP
    and over Tor do not change.
  - Metadata: the relay already saw when a phone went quiet (the connection
    dropped); now it sees missed pings instead. The push server gets the same
    wake-ups as before: one per message burst while the phone sleeps.
  - The terminal and desktop clients change nothing; the relay only holds a
    dead connection of theirs longer before it notices.

### 3. Endpoint rules (the relay must not be turned into a proxy)
- Only `https://`, at most 512 bytes.
- The relay resolves the host and refuses loopback, private, link-local and
  multicast addresses. It connects to the address it checked, so the name
  cannot be re-resolved in between.
- No redirects are followed; the response is read and dropped.
- `SetPush` counts toward the per-key frame budget. A ban drops the account,
  and the endpoint with it.

- **Port 443 only, and no IPv4 hidden in IPv6** (amended 2026-10-19 after the
  external review). An endpoint on another port is refused, so the relay
  cannot be made to knock on other services of a public host. 6to4
  (`2002::/16`) and Teredo (`2001:0::/32`) addresses are refused too, as they
  can carry a private IPv4 address.

## Metadata, honestly

- **The relay** stores one more thing per account, the endpoint URL. It names
  the push server the user chose and a random topic. It names no person.
- **The push server** (ntfy or other) sees the relay's IP and the time of
  each wake-up, so it learns when this endpoint gets messages, but not from
  whom and not what. It also sees the phone's connection to it. A self-hosted
  ntfy keeps that on the user's own server.
- **A network observer** sees an empty POST from the relay to the push server
  right after a message arrives. The same timing is visible today between the
  sender and the relay.
- Without UnifiedPush and with the service off, nothing changes: messages wait
  on the relay (14 days) until the app opens.

The settings screen says all of this next to the switch. Push is off by
default.

## Rejected

- **FCM / Google push:** it needs Play Services, and Google learns the timing
  of every wake-up for every user.
- **Periodic polling with WorkManager:** Android throttles it to 15 minutes
  or more, so messages are late. It still connects regularly, which shows
  the same pattern to an observer.
- **Sending the ciphertext inside the push:** the push server would hold the
  ciphertext. The app has to connect to the relay anyway to acknowledge.

## Dependencies

- **Relay:** no HTTP client library: one empty HTTP/1.1 POST over
  `tokio-rustls` with `webpki-roots`, both already in the tree through the
  client. (Changed at implementation: `reqwest` 0.13 brings `aws-lc-rs`.)
- **App:** the UnifiedPush connector library (`org.unifiedpush.android:connector`).
