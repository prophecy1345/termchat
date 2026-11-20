# ADR-033 — Private groups in the CLI and daemon

Русская версия — [ADR-033](033-scriptable-private-groups.ru.md).

**Status — implemented.** The scriptable mode of ADR-024 and ADR-029 uses
the existing private-group membership, encryption and delivery from ADR-027
and ADR-028. Administration and invitations stay in the app.

`groups` lists groups available to this device. `send` and `recv` can
select one group by exact displayed name or full ID. Both paths pin the
group ID for the invocation and enforce the same argument and access checks.
See the [script guide](../guide/scripts.md#private-groups).

The engine checks access at queueing and transmission. A known move
invalidates pending old-session frames and holds new text until the new
queue and Megolm session are ready. A send succeeds only on the relay's
acknowledgement for its own frame. An expired wait can leave text queued.

The engine marks incoming group texts separately from old history and saves
each read batch before handing it to current subscribers. Subscriber
buffers hold up to 64 records each. A failed save leaves the batch available
to read again. A crash after saving can interrupt output.

[Implementation review and release checks](../security/review-adr-033.md).
