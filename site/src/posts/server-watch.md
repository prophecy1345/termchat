---
title: "When your server goes quiet: termchat watch"
description: "termchat.app 0.18 adds encrypted heartbeats between two hosts. A watcher sends you a private message when the server goes quiet."
date: 2026-11-19
lang: en
---

A backup job can message you when it fails. If the machine loses power, there is nobody left to send that message. With `termchat watch` in 0.18, a second host can notice the silence and write to you.

## One server, one watcher

Give the monitored server and the watcher their own termchat identities. Run them on separate hosts. In this example `backup-box` sends a heartbeat to `watcher` every five minutes. If the watcher goes 15 minutes without a fresh heartbeat, it messages `alice` through her usual termchat app.

Add the server and watcher as contacts, compare their safety numbers, and also verify the alert recipient on the watcher. Run `termchat daemon` on both hosts. Run each command as the same user and with the same `TERMCHAT_HOME` as its daemon.

```sh
# On backup-box
termchat watch send watcher --every 300

# On the separate watcher host
termchat watch monitor backup-box --notify alice --after 900
```

The watcher sends one message for the outage. A fresh heartbeat lets it report the next outage. Both commands run in the foreground on Linux and macOS, so a service manager can restart them when their daemon stops. [Setup guide with contact-pairing steps](/docs/scripts/)

## What a heartbeat tells you

Each heartbeat goes through the existing end-to-end encrypted contact queue. Its send time travels inside the encrypted message. The watcher checks that time so a backlog arriving after a reconnect cannot delay the alert by another full 15 minutes. Heartbeats stay out of chat history and `recv`. An alert is an ordinary private message.

The relay can see the timing of ciphertext, so a steady stream can reveal when the server is active. A relay outage can cause an alert or prevent one from reaching you. Keep the watcher awake and both clocks synchronized. The heartbeat shows that the server's termchat identity was recently reachable over this path. A backup job or web service needs a check of its own. [Design and threat model](https://github.com/prophecy1345/termchat/blob/main/docs/adr/031-watch.md)
