# ADR-007: abuse resistance after the audit

Status: **accepted** with the amendments below (the maintainer delegated the
decision, 2026-10-15). Date: 2026-10-15. Русская версия: [007-abuse-resistance.ru.md](007-abuse-resistance.ru.md).

## Context

An internal review of the M0–M6 code found no holes in the cryptography, but
showed that several defences of a public relay did not work as the project plan
and ADR-006 described. Every item below changes the protocol, limits or security
semantics, so under rule 2 it needs the maintainer's decision. Plain bugs (ack
before saving, paste acting as key presses, a write timeout and so on) were fixed
separately, without an ADR.

## 1. Any stranger can fill a recipient's queue

**Problem.** `Send` checks only that the recipient exists and the sender's rate
limits. The queue quota (1000 envelopes, 10 MiB) is shared per recipient. One
foreign key at 0.5 messages per second fills it in about 35 minutes: everyone
else gets `QueueFull`, and the victim downloads 10 MiB of junk on connect.
Blocking in the client does not help: the relay does not know about it.

**Proposal.** The relay already remembers (in memory) whom each key wrote to
(`Account::recipients`). A pair is **mutual** if the recipient has written to the
sender at least once since the relay started. Non-mutual pairs get a sub-quota:
at most **20 envelopes and 256 KiB** from one sender in one recipient's queue.
Past it, `QueueFull` for that sender only. Mutual pairs are limited by the
overall quota, as before.

This adds a count per `(recipient, sender)` to `queue_stats`; the existing
fields already cover a `queue(recipient, sender)` index.

**Cost.** After a relay restart every pair is non-mutual until the recipient
writes again: until then even a friend is limited to 20 envelopes while you are
offline. Acceptable: 20 messages are enough to notice and reply.

**Rejected.** Keeping the mutuality graph on disk (ADR-005 forbids a social graph
on disk). A per-IP quota (does not work through Tor or CGNAT).

## 2. "Contact requests" do not limit the flow of messages

**Problem.** After the first `ContactRequest` the core creates the contact with
`is_request = true`, but every later `Text` from it is decrypted and shown in
the chat like those of accepted contacts. The "requests" view does not isolate
a stranger. Also, rejecting (Ctrl+D) deletes the contact, and the stranger can
send a new request: the refusal is not remembered.

**Proposal.**
- In `Client::decrypt`, while `is_request == true`, only `ContactRequest` is
  accepted (a repeat updates `note` and `nickname`). `Text` and `ContactAccept`
  are decrypted (the ratchet must move) but return `Error::NotAccepted`; the
  client neither shows nor keeps them, it only counts "N messages wait for
  acceptance".
- A cap on pending requests: **50**. Past it a `ContactRequest` is decrypted (it
  cannot be refused before decryption: the prekey is already spent), but no
  contact or session is created. This bounds the growth of the state file.
- A local list of rejected keys (`Data::rejected: Vec<Ed25519Key>`, capped at
  1000). Ctrl+D on a request adds the key; messages from such keys are not
  decrypted at all (like `blocked`) and the relay gets an `Ack`. The user can take
  the refusal back by adding that person's invite.

**Rejected.** Sending the rejected list to the relay: it reveals the graph.

## 3. A ban through three throwaway keys

**Problem.** Under ADR-006 a report counts if the key ever wrote to the reporter.
Adding a contact from an invite (Ctrl+A) sends a `ContactRequest` by itself. So
someone who added three "public" invites (say, from a "add me" post) could be
banned by their owners without sending a single message.

**Options.**
- A. Count a report only if the reporter **accepted** the contact: the relay
  cannot see that, but it sees traffic in return. Condition: the sender wrote to
  the reporter **at least 3 envelopes** (one `ContactRequest` does not count).
- B. Threshold K = 5 instead of 3.
- C. Reports from keys younger than 7 days weigh half (needs `created_at`, which
  `accounts` already has).

**Proposal.** A + C. Bans stay automatic for real spam (a spammer sends many
messages to many people), and collusion gets more expensive: three week-old keys
to which the victim wrote at least three messages each.

## 4. Live delivery lost on a full channel

**Problem.** If the channel to an online client is full (256 frames), the
envelope stays in the queue, but the client learns of it only on its next
reconnect, which it does not start by itself.

**Proposal.** When `try_send` fails, the relay closes that recipient's
connection. The client reconnects within 1–2 seconds and receives the whole
queue the normal way. No format change. The alternative (`ServerFrame::More`)
needs a new frame and client logic for a rare case.

## 5. Contact name spoofing

**Problem.** The nickname is chosen by the other person. A stranger sends a
request named "Alice", and in the list differs from the real Alice only by the
`?`/`!` mark. Any contact can rename themselves at any time with another
`ContactAccept`.

**Proposal.**
- For unverified contacts (`verified == false`) always show the first 5 digits of
  the fingerprint next to the nickname: `Alice·48213`.
- If a nickname matches another contact's or our own, show the fingerprint for a
  verified contact too.
- `ContactAccept` updates the nickname only once (while `nickname == None`);
  after that the name changes only locally (Ctrl+R).

## 6. Changing the main password

**Problem.** The only way to change the password was to wipe and restore from a
backup. The storage format (ADR-003) allows re-encrypting a slot without
changing the DEK.

**Proposal.** `store::rekey(key: &mut FileKey, new_password)` rewrites the `real`
slot under a new KEK with a new salt; the duress slot is untouched. In the
client: a "current password, new, repeat" form with the same checks as on first
run, plus a distance of at least 4 from the duress password if one is set.

## Consequences

- Protocol (`docs/protocol.md`): section 11 (non-mutual sub-quota), section 7
  (behaviour before a request is accepted). The protocol version does not
  change: the frame format is the same, only behaviour changes.
- ADR-006: the condition for a report to count (item 3).
- Tests: a stranger filling the queue hits the sub-quota, a friend does not; text
  before acceptance is not shown; a rejected key does not come back; three fresh
  keys cannot ban; name spoofing is visible; a password change opens the file
  with the new password and duress still works.

## 7. Amendments after the external review (2026-10-19, maintainer)

- **All strangers together.** The per-stranger cap (20 envelopes, 256 KiB)
  did not stop many fresh keys from filling a queue between them. Envelopes
  from senders the recipient never wrote to now share, together, at most a
  quarter of the recipient's queue, in count and in bytes. Contacts always
  have room.
- **Look-alike letters.** A nickname that mixes Latin, Cyrillic and Greek
  letters (a Cyrillic "А" in "Аlice") always shows the fingerprint digits,
  even for a verified contact. A name written entirely in look-alike letters
  of one other script is not caught. That would need confusable tables, a new
  dependency; it was not chosen.

## Decisions

1. **Non-mutual sub-quota:** 20 envelopes and 256 KiB, as proposed.
2. **Requests:** a cap of 50 pending requests and a rejected list (capped at
   1000), as proposed. **Amendment:** `Text` before acceptance is not thrown away
   but kept hidden, at most 5 per request, and shown in the chat on acceptance.
   Otherwise the first "hi" that follows a request right away would be lost. A
   stranger still cannot flood the screen: anything past 5 is only counted. The
   client does the isolation: the core returns `Received::request = true` and the
   client hides such messages until acceptance.
3. **Reports:** option A + C. The relay counts envelopes per pair (in memory); a
   report counts with at least 3 envelopes from the sender to the reporter; a
   reporter key younger than 7 days weighs 0.5; a ban at a total weight of 3.
4. **Full channel:** the relay drops the recipient's connection; no separate
   frame.
5. **Nickname:** as proposed. A repeated `ContactRequest` updates the nickname
   while the contact is still a request; after acceptance the nickname no longer
   changes from the network.
6. **Password change:** **amendment**: "distance of at least 4 from the duress
   password" cannot be checked, since the client does not know the duress
   password, only its slot. Instead, the new password must not open the duress
   slot (otherwise it is the duress password), and the form warns that a typo
   close to the duress password could erase everything.
