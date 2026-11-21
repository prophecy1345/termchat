# ADR-037 — Comparing safety numbers with a QR code

Status — **accepted by the maintainer**. [Русская версия](037-safety-number-qr.ru.md).

## Context

A contact is verified when both people compare their 60-digit safety number out of band (protocol, section 10). Reading 60 digits aloud is tedious, so many people skip it, and an unverified contact is exactly where a relay or network attacker could sit in the middle. Signal and others let one phone scan the other's code instead.

## Decision

1. **What the code holds.** `TERMCHAT-SAFETY:` followed by the 60 digits of the pair's safety number, without spaces: capitals and digits only, so the code uses the QR alphanumeric mode and stays small enough for a terminal. A scan accepts the prefix in either case. The number is already public between the two people and symmetric, so both sides show the same code. It names both identity keys through their fingerprints; it holds no key, address or name.
2. **Who shows it.** The safety-number window shows the code next to the digits: Android, the desktop app and the terminal (as Unicode half blocks, through the `qrcode` crate the desktop app already uses).
3. **Who scans.** Android, with the camera scanner it already has for invites, from the safety-number window of the open contact.
4. **What a scan does.** The phone computes the safety number of the open contact and compares it with the scanned one, digit for digit.
   - Equal: the contact is marked verified on the phone, and the window says so. The other side is not changed: it verifies on its own, by scanning back or comparing the digits.
   - Different, or not a `TERMCHAT-SAFETY:` code: a clear warning that it does not match, and nothing changes. A previously verified contact stays verified; the warning tells the person to find out why before trusting the chat.
5. Comparing the digits by eye stays, and so does **v** to mark a contact verified by hand.

## Threat model

- The comparison is as strong as the one done by eye, under the same rule: only a code shown on the other person's own screen, in front of you, proves anything. A code received through the chat itself, a screenshot or a photo someone sends proves nothing, and the guide says so.
- A second-preimage search for a code that matches someone else's pair needs about 2^99 tries per fingerprint (protocol, section 10).
- The code is not secret, but it shows who talks to whom to anyone who photographs it with knowledge of both fingerprints; show it only for the comparison.
- Scanning never unverifies, so a wrong code from someone else cannot make a verified contact look unverified, and cannot verify a contact either.

## Alternatives considered

- **Encode both public keys.** Same strength, longer code, and nothing the safety number does not already pin.
- **Verify both sides from one scan.** That would need a message back to the shown side; a single local decision keeps the protocol unchanged.
- **Scan on the desktop too.** Most computers have no camera worth it; showing the code there covers the phone-and-computer case.

## Acceptance criteria

The code of a pair decodes to its 60 digits on every client. A scan of the same pair marks the contact verified on the scanning phone only. A scan of another pair, a code with another prefix, or garbage shows the mismatch warning and leaves the verified state as it was. The terminal draws a code a phone camera reads at a normal terminal size.
