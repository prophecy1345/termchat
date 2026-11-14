# ADR-022: locking a frozen Android app, and three relay limits

Status: **accepted** (maintainer, 2026-11-14). Русская версия: [022-review-hardening.ru.md](022-review-hardening.ru.md).

## Context

The second source review ([review-2026-11b](../security/review-2026-11b.md))
left two findings that change documented behaviour.

- **Finding 2, the rest.** Without "stay connected", ADR-011 locks the app
  after the chosen time in the background. The check runs on a timer inside
  the process, and Android freezes a background process within seconds; a
  frozen one checks nothing. The keys stay in memory until the user comes
  back (which now locks at once, 0.6.2) or Android kills the process.
- **Finding 6.** Three ways to load the relay: logins of an existing key are
  not rate-limited; the report journal has no size cap; a slow reader can
  hold 256 frames (up to 16 MiB) in its delivery channel.

## Decision

### 1. An alarm locks the backgrounded app (Android, "stay connected" off)

- Going to the background schedules an alarm with `AlarmManager`
  (`setAndAllowWhileIdle`, wall clock) for the moment the lock time runs
  out. Coming back cancels it.
- The alarm wakes the process, unfrozen, and runs the usual check, which
  locks and drops the keys.
- No new permission. The alarm is inexact: Android may fire it some minutes
  late, and in deep sleep about once in 9 minutes at most. Exact alarms
  need a permission the user grants by hand; not worth it.
- With "stay connected" on nothing changes: the app stays unlocked in the
  background by choice (ADR-012).

### 2. Relay limits

- **Logins per key:** 30 at once, then one a minute (`Rate`). Past that the
  login gets `RateLimited` and the client backs off as after any refusal. A
  phone reconnecting on every push stays far below it.
- **Report journal:** at most 10 MiB. Past that a report is still counted
  (bans work as before) but its text is not written; the relay logs once
  that the journal is full. The operator reads and empties it.
- **Delivery channel:** 32 frames (the queue page size), not 256. A reader
  that falls behind is dropped, reconnects and gets the queue page by page,
  as today when the channel is full.

## Not done

- **Finding 9** (`KeyCount` makes the client publish keys): only the relay
  can send it, and the relay can deny service anyway. No change.

## Consequences

- A backgrounded app without "stay connected" no longer holds its keys for
  hours, at the cost of an alarm per trip to the background.
- The relay's memory per connection and disk for reports are bounded.
