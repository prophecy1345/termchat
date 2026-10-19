# ADR-012: waking the Android app for new messages

Status: **accepted** (maintainer, 2026-10-17; section 2 amended the same day). Русская версия: [012-push.ru.md](012-push.ru.md).

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
