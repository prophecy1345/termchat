# ADR-016: an RSA release key for Android

Status: **accepted** (maintainer, 2026-10-20). Amends ADR-011. Русская версия: [016-rsa-release-key.ru.md](016-rsa-release-key.ru.md).

## Context

The Android release key from ADR-011 is an EC P-384 key (SHA-256
`BC:6A:74:2E:…:BA:5E`). Google Play accepts only RSA keys of 2048 bits or more,
both as the app signing key and as the upload key. Uploading ours through PEPK
fails, and an app bundle signed with it would be refused.

We want one key everywhere: GitHub, F-Droid (which ships our APK only if its
rebuild matches, ADR-011) and Google Play. Then an install from any of them
updates from any other.

No one has installed a release yet, so the key can change now without breaking
updates. Later it could not, short of a v3 rotation lineage.

## Decision

1. A new release key: RSA 4096, SHA256withRSA, `CN=termchat`, alias `termchat`,
   valid 100 years, in a PKCS12 keystore kept outside the repository like the
   old one. SHA-256 `9F:FB:61:1C:DD:1B:3A:AE:63:30:3F:B7:12:0B:85:F7:67:0D:08:BD:B8:24:D6:7C:7C:85:37:09:88:0C:2A:7F`.
2. It signs everything from the next release on: the APKs and the Play bundle.
   The old EC key signs nothing more. It is kept offline, not destroyed: it
   proves who published releases up to 0.3.2.
3. Play App Signing gets the same key through PEPK, so Play-delivered APKs carry
   the same certificate as ours.
4. No rotation lineage: there are no installs to carry over. Anyone who
   installed 0.3.2 or older must uninstall before installing the new release.
5. The new fingerprint replaces the old one in README.md, README.ru.md and
   `AllowedAPKSigningKeys` in the F-Droid recipe. The release notes say the key
   changed and why.

## Consequences

- The key is the same on all three channels, so users can switch between them.
- Google holds a copy of the signing key (Play App Signing). Google could sign
  an update for Play users, and also an APK that sideloaded installs would
  accept. That trust exists for every app on Play; users who do not want it
  install from F-Droid or GitHub and compare the fingerprint, as before. It is
  stated in the threat model.
- RSA 4096 signatures are larger than EC ones; it does not matter for an APK.
