---
title: "First run of termchat.app: nickname, password, relay, Tor"
description: "Create your termchat.app identity: a nickname, a strong password, the relay and whether to use Tor. No phone number or email."
---

# First run

Русская версия: [first-run.ru.md](first-run.ru.md).

The first start makes your identity: a key pair on this device, sealed with your password. Nobody registers you anywhere; there is no phone number and no email.

## What you choose

| Field | What it is |
|---|---|
| Relay URL | Filled in with the project's public relay, `wss://relay.termchat.app/`. Keep it, or type the address of [your own relay](../self-host.md). |
| Relay onion URL | The same relay as a Tor onion address. Needed only with Tor; for another relay, its own onion address or nothing. |
| Use Tor | Yes hides your IP address from the relay and from your internet provider. The first connection then takes 10 to 30 seconds. No connects directly and is faster. See [Tor and bridges](tor.md). |
| Nickname | What your contacts see. It is the only thing anyone learns about you; no real name needed. Up to 64 bytes. |
| Password | Encrypts everything on your disk. Make it strong, and write it down: it cannot be recovered. |
| Repeat password | The same again. |

Each app can make a strong password for you: six random words.

## In the terminal

Run `termchat`. The setup window shows the fields in the order above; **Tab** or **Enter** moves to the next one, **Shift+Tab** back. **Ctrl+G** fills both password fields with six random words and shows them once: write them down. **Ctrl+B** restores from a backup instead.

## In the desktop app

Fill in **NICKNAME**, **PASSWORD** and **PASSWORD AGAIN**, tick **Connect through Tor** if you want it, and press **Create my identity**. The relay addresses are under **Relay**. **Restore from a backup** is below.

## On Android

Fill in the fields and press **Create**. **Use Tor** switches with a tap; **Generate a passphrase** fills in the password and shows it with a **Copy** button. **Restore** takes a backup file instead.

## Right after

The app suggests making a backup at once, and keeps a reminder ("no backup") until you do: without one, a lost device means a lost identity. See [Backups, locking, wiping](safety.md). Then [add your first contact](contacts.md).
