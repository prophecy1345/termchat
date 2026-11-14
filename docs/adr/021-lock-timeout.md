# ADR-021: choosing when the app locks itself

Status: **accepted** (maintainer, 2026-11-14). Русская версия: [021-lock-timeout.ru.md](021-lock-timeout.ru.md).

## Context

ADR-011 locks the Android app after 5 minutes without a touch, on screen or in
the background; ADR-012 lifts the background part while "stay connected" is on.
A phone whose screen stays on for long (a screen timeout of 30 minutes is a
system choice) locks the app in front of its user every 5 minutes, and the
password has to be typed again. Five minutes is a fixed guess; it fits a phone
on a desk in an office worse than one in a pocket.

## Decision

- Settings gets **Lock after**: 1, 5, 15 or 30 minutes without a touch.
  The default stays 5 minutes, as in ADR-011.
- **No "never".** A phone left unlocked with the app open must lock at some
  point; 30 minutes is the ceiling.
- The time applies on screen and in the background. "Stay connected" still
  keeps a backgrounded app unlocked (ADR-012); the on-screen lock is not
  affected by it.
- The choice is kept in the plain settings, beside "stay connected" and the
  theme (ADR-008): the lock screen needs it before the password. It says
  nothing beyond what those settings already say: that termchat is in use.
  A wipe deletes it with them.
- Locking by hand and the duress password are unchanged.
- All three clients. Android keeps it in its plain settings; the terminal and
  desktop clients in `settings.json` beside the state (ADR-008), which they
  share. A value edited by hand outside the choices is held to 1-30 minutes.

## Consequences

- A longer time gives whoever picks up an unlocked phone more time to read the
  chats. The settings screen says so next to the choice.
- The user decides the trade-off; the default does not change for anyone who
  does not look.
