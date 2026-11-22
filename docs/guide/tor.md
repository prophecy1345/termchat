---
title: "Tor in termchat.app: hide your IP, bridges where Tor is blocked"
description: "How termchat.app uses Tor to hide your IP address from the relay and your provider, a circuit per contact, and obfs4, webtunnel and snowflake bridges where Tor itself is blocked."
---

# Tor and bridges

Русская версия: [tor.ru.md](tor.ru.md).

Tor is built in: nothing else to install.

## What it does

Without Tor, the relay and your internet provider see your IP address, and your provider sees that you talk to the relay. With Tor on:

- the app connects only through Tor, to the relay's onion address;
- the relay does not see your IP address, and your provider sees only that you use Tor;
- each contact gets a Tor circuit of its own (see "Connections" below).

The first connection takes 10 to 30 seconds; later ones are quicker. Tor needs the relay's onion URL. The public relay has one, filled in for you; another relay may not.

## Turning it on or off

- At [first run](first-run.md): the "Use Tor" choice.
- Later: settings → security → **Tor** in the terminal, **Turn Tor on/off** in the desktop app, **Settings → Tor** on Android.
- Command line: `termchat init --tor` for a new identity.

## Connections: one per contact, or one for all

By default each contact gets a connection of its own, and with Tor a circuit of its own, so the relay cannot tell which queues are yours by the connection they come on. That costs battery and network.

**One connection for all contacts** uses less of both, but the relay then sees your queues together: it learns which queues belong to one person, though still not who that is. In settings: **one connection for all contacts** (terminal), **One connection for all contacts** (desktop), **Connections** (Android).

## Right after unlocking

With Tor, your contacts' connections come up one by one over the first 30 seconds after you unlock, so the relay cannot tie them together by the second they arrive. Until a chat's connection is up, its header says **connecting…**: messages to and from it wait, and nothing is lost. Without Tor the relay sees one IP address for all of them anyway, so they connect at once.

**Connect the open chat at once** lets the chat you open skip that wait. The relay may then see which chat you opened right after unlocking. It is off by default. In settings: **connect the open chat at once** (terminal), **Connect the open chat at once** (desktop and Android).

## Bridges, where Tor is blocked

Some countries and networks block Tor. Bridges are unlisted entry points behind a disguise:

- **obfs4** looks like random noise;
- **webtunnel** looks like an ordinary HTTPS website;
- **snowflake** goes through volunteers' browsers and looks like a video call.

1. Get bridge lines at [bridges.torproject.org](https://bridges.torproject.org) or from @GetBridgesBot in Telegram.
2. Open **Tor bridges** in settings (security tab in the terminal, **Tor bridges** in the desktop app, **Settings → Tor bridges** on Android).
3. Paste them, one per line. In the terminal, paste or type one and press **Enter**, then **Ctrl+S** saves; **Ctrl+X** clears.

With bridges set, Tor enters the network only through them, by way of lyrebird, the Tor Project's transport program, which ships next to `termchat` and in the Android app. The network sees the disguise, not Tor. The bridge sees your IP address, as any first Tor hop does.

Through bridges the first connection can take a few minutes.
