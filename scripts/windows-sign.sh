#!/usr/bin/env bash
# Tauri's `bundle.windows.signCommand` for the cross-built Windows binaries
# (installer, uninstaller, NSIS plugin DLLs). Tauri passes one file path.
#
# With a certificate, WINDOWS_SIGN_PFX (+ WINDOWS_SIGN_PASSWORD) names a
# PKCS#12 file and the binary is Authenticode-signed with osslsigncode, which
# also fills in the PE checksum. Without one, only the checksum is set
# (scripts/pe-checksum.py): an unsigned file with a zero checksum is one more
# thing antivirus heuristics count against it.
#
# The uninstaller is finalised by makensis itself, so for the Docker-backed
# makensis this runs inside the container: keep it POSIX and self-contained.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -n "${WINDOWS_SIGN_PFX:-}" ]; then
  command -v osslsigncode >/dev/null 2>&1 || {
    echo "windows-sign: osslsigncode not found (brew install osslsigncode)" >&2
    exit 1
  }
  for f in "$@"; do
    osslsigncode sign -pkcs12 "$WINDOWS_SIGN_PFX" \
      ${WINDOWS_SIGN_PASSWORD:+-pass "$WINDOWS_SIGN_PASSWORD"} \
      -n "Socorin" -i "https://socorin.com" \
      -h sha256 -t "${WINDOWS_SIGN_TIMESTAMP_URL:-http://timestamp.digicert.com}" \
      -in "$f" -out "$f.signed"
    mv -f "$f.signed" "$f"
  done
else
  python3 "$ROOT/scripts/pe-checksum.py" "$@"
fi
