---
title: "The sleeping link: delivering to a dozing Android phone over Tor in 2 seconds, not 15"
description: "Why the first message to a sleeping Android phone took 10-15 seconds over Tor, what the logs showed, and how keeping the phone's connection open while it sleeps cut it to about 2 seconds without a wakelock."
date: 2026-11-14
lang: en
---

termchat.app is an end-to-end encrypted messenger for the terminal, the desktop and Android. Messages go through a relay; on Android you can reach it over Tor, through the relay's onion address. A user reported that with the phone asleep, the first message arrived 10 to 15 seconds after it was sent. Later ones were quick. This is how we found out where those seconds went, and what changed in 0.6.1.

## The setup

- The app keeps a WebSocket to the relay. With "stay connected" on, a foreground service keeps the process alive in the background.
- The relay pings every 30 seconds and closed a connection after 60 seconds without a word from it.
- When the relay has a message for someone it holds no connection of, it sends an empty wake-up through [UnifiedPush](https://unifiedpush.org/) (here: ntfy). No content, no sender, only "something waits".
- Woken up, the app dials the relay again and fetches its queue.

## Where the seconds went

First suspect: the push itself. It was not. A message sent at 14:40:00 produced the notification at 14:40:01; the push server and Android delivered it in about a second.

Second: the connection. While the phone slept, the app had no connection to the relay at all, though Android allowed it the network (`dumpsys netpolicy` showed the foreground service exempt from Doze's firewall, and `dumpsys connectivity` that its sockets survive Doze entry). Its TCP connections to Tor guards were alive; the onion circuit to the relay was gone.

To see when it went, we watched the relay's onion port from the server, `ss` every two seconds, while the phone lay still:

- 14:43:52: the phone entered deep Doze (`deviceidle`: `IDLE`).
- 14:45:02: the relay closed the phone's connection. 70 seconds later: one missed ping, then the 60-second limit.

In deep sleep Android stops the processor. The phone's kernel still takes in the relay's pings and acknowledges them at the TCP level, but the app does not run, so nobody answers with a pong. After a minute the relay concluded the phone was gone. The next message then needed a push, and the app a new connection: under Tor, a new circuit to an onion service, which is where 10 to 15 seconds went.

## What we did not do

- **A wakelock.** Keeping the processor awake while "stay connected" is on would keep the connection healthy. It would also drain the battery, more so with Tor's background traffic.
- **Periodic alarms.** In Doze, Android lets an app's alarm fire at most about once in 9 minutes. That does not help with seconds.
- **Pinging from the phone.** It needs the processor, which is exactly what is missing.

## The sleeping link

The push already wakes the phone reliably: the push distributor is on Android's battery allowlist and keeps its own connection. What was slow was only the new connection. So the relay now keeps the old one.

1. **The relay holds a silent connection for up to an hour**, not 60 seconds, and keeps pinging it.
2. **A connection that did not answer a ping by the next one counts as asleep.** A message for it goes into the connection as usual, **and** the relay sends a push, at most one per 30 seconds per identity.
3. **The phone wakes up, and the message is already in its socket.** The app reads it from the open connection. No new circuit.
4. **Then it checks the link**: it sends its own ping. If nothing comes from the relay within 5 seconds, it drops the connection and dials again, as before. The same check replaces dropping the link outright when the app comes on screen after 35 seconds of silence.

Nothing is lost if the held connection turns out to be dead: a message leaves the relay's queue only when the client acknowledges it, so the next connection gets it.

## The result

The same test with 0.6.1:

- 15:09:00: message sent from the desktop app.
- 15:09:01.7: the push server received the wake-up and handed it to the app.
- 15:09:02.1: the notification on the phone.
- On the relay: the same connection as before. No new one.

From 10-15 seconds to about 2, with the phone in deep Doze and no wakelock.

## The costs

- **The relay holds dead connections longer**: up to an hour instead of a minute. It holds at most one per identity, since a new connection replaces the old, and the limits on connections per IP and over Tor are unchanged.
- **The relay sees missed pings.** Before, it saw the connection drop, which says the same: this phone is asleep.
- **The push server sees what it saw before**: one empty wake-up per burst of messages while the phone sleeps.
- **Both ends had to change.** A new app with an old relay waits 5 seconds for an answer before dialling again, so the relay update shipped first.

The decision is written up as an amendment to [ADR-012](https://github.com/prophecy1345/termchat/blob/main/docs/adr/012-push.md), with the measurements; the code is in [`crates/relay/src/lib.rs`](https://github.com/prophecy1345/termchat/blob/main/crates/relay/src/lib.rs) and [`crates/engine/src/relay.rs`](https://github.com/prophecy1345/termchat/blob/main/crates/engine/src/relay.rs). termchat.app has had no independent security audit yet; treat it as experimental.
