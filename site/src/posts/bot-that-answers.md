---
title: "A bot that answers within a second: termchat daemon for servers"
description: "termchat.app 0.14 adds termchat daemon: a server identity stays open, so scripts send and read without a password per command and a bot answers your phone within a second, end to end encrypted, with nothing listening on the network (ADR-029)."
date: 2026-11-18
lang: en
---

Since 0.8, a script can write to your phone with `termchat send` and read your answer with `termchat recv`: "the backup failed", end to end encrypted, without a bot account. That works well for a notice now and then. For a conversation with your server it was slow. Version 0.14 fixes that with `termchat daemon`.

## Why it was slow

Each command did everything from the start: it unlocked the identity with its password, connected to the relay, did its one thing and exited. Over Tor a new connection takes 10 to 30 seconds. So a bot that answers "status" from your phone took half a minute per answer, every script needed the password file, and two commands at the same moment could not share one identity: the second one failed.

## What the daemon does

`termchat daemon` unlocks the identity once and stays: its connection to the relay is already open, and the engine's timers run as in the app. While it runs, the usual commands go through it by themselves:

```sh
termchat daemon --password-file /etc/termchat-bot/pw &
termchat send alice "disk at 95 %"        # no password, no new connection
termchat recv --from alice --follow --json
```

The scripts do not change. If no daemon runs, the commands work as before. On our own server, `termchat contacts` through the daemon takes 0.01 seconds.

## A bot that answers

The relay that serves termchat.app has its own identity, relay-box, which sends its owner a morning report. Now it also answers:

```sh
termchat recv --from alice --follow --json | while read -r line; do
  case "$(printf '%s' "$line" | jq -r .text)" in
    status) /usr/local/bin/termchat-report ;;
    disk)   df -h / | termchat send alice ;;
    *)      termchat send alice "I answer: status, disk" ;;
  esac
done
```

Write "status" on the phone and the report is back within a couple of seconds. The bot answers only its owner and only questions that change nothing on the server: whoever gets hold of your phone must not get a console on your server with it.

The guide has [a systemd unit and the details](/docs/scripts/).

## What it costs, honestly

- **The password and the keys stay in memory** while the daemon runs. Whoever gets root on the server, or its user, reads and writes as this identity. That was already true of the password file next to the state: give the daemon a server identity under a user of its own, never your main one.
- **Every process of that user can send through it.** The daemon listens on a Unix socket in the private state folder, mode 600, and the kernel tells it who connects: other users are turned away. Nothing listens on the network; there is no port and no HTTP.
- **The daemon never opens a path it is given.** A document to send is read by the command, as its own user; one to save goes into a folder you name.
- **No broadcasts, no accepting strangers**, as before: one message to one contact, and a server identity cannot let anyone in by itself.

The daemon runs on Linux and macOS; Windows has no Unix sockets for it. It is not a new cryptographic protocol: the same engine, sessions and queues, only a shorter way for commands to reach them. The decision and what was rejected are in [ADR-029](https://github.com/prophecy1345/termchat/blob/main/docs/adr/029-daemon.md).
