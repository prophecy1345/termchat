# ADR-003: duress password

Status: accepted; section "Backups on this device" accepted 2026-10-17. Date: 2026-10-15. Русская версия: [003-duress-password.ru.md](003-duress-password.ru.md).

## Context

A user's device is seized and a password is demanded. We want a second password
that irreversibly destroys the data, after which the app looks freshly
installed.

## Decision

### File format: a data key and two slots

```
Header {
    version: u16,
    argon2: { m_kib: u32, t: u32, p: u32 },  // shared by both slots
    slots: [Slot; 2],          // always two, in random order
}
Slot {
    salt: [u8; 16],            // for Argon2id
    nonce: [u8; 24],
    wrapped: [u8; 48],         // XChaCha20Poly1305(KEK, 32 bytes) + 16-byte tag
}
Body: XChaCha20Poly1305(DEK, data)
```

- Data is encrypted with a random key, the **DEK**, not with the password key.
- **Main password slot:** `wrapped` = the DEK encrypted with
  `KEK = Argon2id(password, salt)`.
- **Duress password slot:** `wrapped` = a fixed 32-byte marker
  `termchat-duress-marker-v1`, encrypted with the key from the duress password.
  Duress is recognised only by this marker. (Refined in M4: the first idea, "a
  slot opened but the body did not decrypt, so it is duress", would have erased
  the data of a corrupted file opened with the right main password. That case is
  now `CorruptState`, with no erasing.)
- **No duress password set:** the second slot is filled with random bytes. It
  cannot be told apart from a real slot: AEAD ciphertext looks random.
- Changing a password re-encrypts only its slot; the data is not touched.

### Checking a password
- Argon2id is **always computed for both slots**; the decision is made only after
  both.
- The file body is decrypted exactly once on every path; if no slot opened, a
  dummy key is used.
- Remaining difference (found by a test in M4): on success the AEAD also runs the
  ChaCha20 keystream over the body; on failure it stops at the tag check. The
  difference grows with the file size: a few milliseconds per megabyte against
  about a second of Argon2. Accepted: after the duress password erasing starts at
  once, and that is visible from the behaviour anyway.
- The slot order is random, so the position gives nothing away either.
- Cost: unlocking takes twice as long. With 64 MiB and 3 iterations that is a
  fraction of a second.

### Destruction (crypto-erase)
When the duress password is entered, before anything is shown:
1. The whole file is overwritten with random bytes, `fsync`ed and deleted.
2. Leftovers on SSDs: the controller may write the new bytes elsewhere, and old
   copies of the file (from every save) live on until TRIM. They can be read only
   with the **main** password, which the duress password does not reveal. So
   against being forced to give the duress password, erasing works. A lab that
   images the flash chips and later learns the main password could recover the
   data. Protection: the OS's disk encryption (FileVault, BitLocker, LUKS).
3. Keys are wiped from memory (`zeroize`).
4. The interface settings file and the Tor directory (`arti/`, ADR-004) are
   deleted too: the first holds nothing secret, the second shows that termchat
   was used over Tor. Arti may still flush its state while the wipe runs, so
   the client deletes the directory again on the next start that finds no
   state file; a fresh install has nothing there.
5. The client shows the first-run screen, with no "data erased" or "wrong
   password" message.
6. Nothing goes to the network: no goodbye to contacts, no `Unregister`
   (ADR-009). The relay forgets the key once it has been idle for 90 days.

The same works on the auto-lock screen.

### Backups on this device (accepted 2026-10-17)
A backup left on the same disk survives the erase, opens with a password that
can be forced out too, and shows that termchat was used. So:

1. **The client remembers where backups are.** Writing a backup (Ctrl+P, b)
   and restoring from one (Ctrl+B) add a line to `backups` next to the state
   file: the file's path and the salt of its first slot (16 random bytes, in
   the file's header anyway). The duress password does not open the state
   file, so this list has to be readable without the main password: it is a
   plain file, mode 0600.
2. **Every wipe erases them:** duress, `WIPE` and `MOVED` (ADR-009) alike. For
   each line, if the file at that path exists and its header carries the same
   salt, it is overwritten, `fsync`ed and deleted, like the state file; then
   `backups` itself goes the same way. A missing file, a different file at the
   path or an unmounted USB stick is skipped silently. The salt check keeps a
   planted line from erasing an unrelated file.
3. **No default path.** The backup form no longer offers `~/termchat-backup`:
   the user types where it goes.
4. **A warning after writing:** "Move it off this device. A wipe erases it only
   while it stays at this path."

What this does not do, honestly:
- A copy the user moved, renamed or copied elsewhere is not known and not
  touched. That is the point: it is how they get their identity back.
- A backup on a USB stick that is plugged in during the wipe is erased.
- `backups` names where backups live (for example `/Volumes/USB/tc`) to
  anyone who reads the disk before a wipe. It holds no key material.
- On an SSD the overwritten blocks may survive until TRIM (point 2 above);
  they are encrypted with the backup password.

### Settings
- Off by default; turned on in the settings.
- When turning it on, the client shows the warning below and asks for
  confirmation.
- The duress password must differ clearly from the main one: not a prefix, not
  one character apart, or a typo would destroy the data. Checked with Levenshtein
  distance (at least 4) and `zxcvbn`.

## Limitations (stated in the README and in the warning)

1. **No help against a copied disk.** If a copy was made before the password was
   entered, the main password opens the copy. It works only when the password is
   typed on the device itself.
2. **The code is open.** An adversary knows about the feature. An empty app after
   the password can reveal that it was a trap.
3. **Legal risk.** In many countries destroying data after a seizure is
   punishable as destroying evidence or obstruction. Users decide for themselves.
4. **Physical risk.** Having found the deception, the adversary may increase the
   pressure.
5. **Backups elsewhere.** Only backups at the path the client wrote or restored them from are erased (see "Backups on this device"); copies moved elsewhere survive.
6. **The relay and contacts** do not learn about the erasing. Messages that
   arrive later have no one to decrypt them.

## Rejected

- **A decoy profile** (a second password opens a fake profile). Stronger against
  coercion, but three to four times more complex: fixed-size files, real data
  indistinguishable from noise, a "lived-in" fake profile. Possible later; the
  two-slot format allows it.
- **Plain deletion without crypto-erase.** On SSDs the data physically remains.

## Consequences

- Changes the storage format of ADR-002: a DEK with two slots instead of a
  password-derived key. ADR-002's storage sections were updated.
- Implemented in M4. Tests: both passwords take the same time (within noise);
  after the duress password the main password no longer opens the file; a file
  without duress is structurally indistinguishable from one with it.
