#!/usr/bin/env bash
# Cross-compiles the Windows x64 NSIS installer from macOS or Linux.
# One-time setup (macOS): brew install llvm lld
#                          rustup target add x86_64-pc-windows-msvc
#                          cargo install cargo-xwin
#                          Docker (for makensis, see below)
# cargo-xwin downloads the Windows SDK/CRT headers on first use (~1 GB cache).
# Code signing: put the certificate settings in .env.windows (see
# .env.windows.example). Without it the binaries stay unsigned and
# scripts/windows-sign.sh only fills in their PE checksum.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
for f in llvm lld; do
  BIN="$(brew --prefix "$f" 2>/dev/null)/bin"
  [ -d "$BIN" ] && export PATH="$BIN:$PATH"
done
# Homebrew's makensis crashes on Apple Silicon (std::bad_alloc, 3.12 included);
# use the Docker-backed wrapper instead. It builds the official NSIS release,
# whose installer stubs, unlike the Ubuntu package's, are not flagged as
# corrupt by PE parsers and antivirus engines (see docker/Dockerfile.nsis).
# A shim, not a symlink: the wrapper finds the project from its own path
# ($0), which a symlink in a temp dir would hide.
if [[ "$(uname)" == "Darwin" ]] && command -v docker >/dev/null; then
  TMPBIN="$(mktemp -d)"
  printf '#!/bin/sh\nexec "%s" "$@"\n' "$ROOT/scripts/makensis-docker" > "$TMPBIN/makensis"
  chmod +x "$TMPBIN/makensis"
  export PATH="$TMPBIN:$PATH"
  # The bundler copies NSIS's stock plugins from $NSIS_PATH, or from
  # share/nsis next to makensis, which for the shim is a temp dir: export the
  # image's share/nsis (same NSIS version as the compiler) and point it there.
  NSIS_SHARE="$HOME/Library/Caches/tauri/nsis-share"
  "$ROOT/scripts/makensis-docker" --export-share "$NSIS_SHARE"
  export NSIS_PATH="$NSIS_SHARE"
fi
cd "$ROOT"
if [ -f .env.windows ]; then
  set -a; . ./.env.windows; set +a
fi
# The updater signing key (see .env.updater.example): the bundler writes a
# .sig next to the installer, which the in-app updater checks before it
# runs the installer.
if [ -f .env.updater ]; then
  set -a; . ./.env.updater; set +a
fi
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "warning: TAURI_SIGNING_PRIVATE_KEY is not set (.env.updater); the in-app updater cannot install this build." >&2
else
  TAURI_SIGNING_PRIVATE_KEY="${TAURI_SIGNING_PRIVATE_KEY/#\~/$HOME}"
  export TAURI_SIGNING_PRIVATE_KEY
fi
npm run tauri build -- --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
mkdir -p installers
# Only this build's installer (and its updater signature): the bundle dir
# keeps installers of older names.
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
cp -v src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/Socorin_"$VERSION"_*.exe installers/
cp -v src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/Socorin_"$VERSION"_*.exe.sig installers/ \
  || echo "warning: no updater signature was produced (TAURI_SIGNING_PRIVATE_KEY missing?)" >&2
# The sign command ran on the installer (a signed file has a valid checksum
# too); a zero checksum means Tauri skipped it. The app inside was signed the
# same way (Tauri logs "Signing .../socorin.exe"); the copy left in target/
# is the pristine one Tauri restores after bundling, so it is not checked.
scripts/pe-checksum.py --check installers/Socorin_*.exe
