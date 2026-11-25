#!/bin/sh
# Prints the SHA-256 of every entry of an APK except the v1 signature files,
# so a signed APK can be compared with an unsigned build of the same commit
# (v2/v3 signatures sit outside the entries).
# Usage: scripts/apk-entries.sh <apk>
set -eu
python3 -I - "$1" <<'PY'
import hashlib, re, sys, zipfile
sig = re.compile(r"META-INF/([^/]+\.(SF|RSA|DSA|EC)|MANIFEST\.MF)")
with zipfile.ZipFile(sys.argv[1]) as z:
    for n in sorted(z.namelist()):
        if not sig.fullmatch(n):
            print(hashlib.sha256(z.read(n)).hexdigest() + "  " + n)
PY
