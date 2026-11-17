---
title: "termchat.app documentation: install, contacts, messages, Tor, scripts"
description: "How to use termchat.app, the end-to-end encrypted messenger for the terminal, the desktop and Android: install, first run, contacts, messages, Tor, backups, scripts and your own relay."
---

# Documentation

termchat.app is an end-to-end encrypted messenger for text and documents. The same account works in a terminal, in a desktop app for macOS, Linux and Windows, and on Android. There is no phone number and no email: you are a key pair made on your device and a nickname only your contacts see.

These pages say how to use it, step by step. Each section stands on its own; the menu takes you to any of them.

## Where to start

- New here: [Install](install.md), then [First run](first-run.md), then [Contacts](contacts.md).
- Moving to a new device, or worried about losing it: [Backups, locking, wiping](safety.md).
- Messages from a server or a script: [Scripts and servers](scripts.md).
- Your own server for your friends: [Run a relay](../self-host.md).

## What it protects, and what it does not

The relay passes along envelopes it cannot open and does not know who talks to whom. It still sees when a queue is written to and read, rough message sizes, and your IP address unless Tor is on. The [threat model](../threat-model.md) lists all of it. The app has not had an independent security audit yet; the encryption library it uses, vodozemac, has.
