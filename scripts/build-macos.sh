#!/usr/bin/env bash
# Build the macOS installer signed with a Developer ID certificate and
# notarized, so users open it without Gatekeeper warnings.
#
#   scripts/build-macos.sh              # universal (Intel + Apple Silicon)
#   scripts/build-macos.sh aarch64      # Apple Silicon only (faster)
#   scripts/build-macos.sh x86_64
#
# Credentials come from .env.apple (see .env.apple.example) or the environment:
#   APPLE_SIGNING_IDENTITY                                  signing
#   APPLE_API_KEY + APPLE_API_ISSUER + APPLE_API_KEY_PATH   notarization, or
#   APPLE_KEYCHAIN_PROFILE   a profile made once with `xcrun notarytool
#                            store-credentials <name> --apple-id <email>
#                            --team-id <TEAMID>` (password typed at the prompt)
# Without notarization credentials the app is only signed (Gatekeeper still
# warns); without a signing identity it is ad-hoc signed like a plain build.
#
# The build runs in two phases so the credentials reach as little as
# possible: the frontend and cargo build first, without them (`tauri build
# --no-bundle`); then only `tauri bundle`, which packages the binary that
# already exists and never runs cargo, npm or Vite, with them loaded.
set -euo pipefail
cd "$(dirname "$0")/.."

ARCH="${1:-universal}"
case "$ARCH" in
  universal)     TARGET=universal-apple-darwin ;;
  aarch64|arm64) TARGET=aarch64-apple-darwin ;;
  x86_64|x64)    TARGET=x86_64-apple-darwin ;;
  *) echo "unknown arch: $ARCH (universal | aarch64 | x86_64)" >&2; exit 2 ;;
esac

# Spotlight would otherwise list the .app bundled below next to the
# installed one. The bundle is moved into a folder Spotlight skips when
# this script ends, however it ends (the helper says why nothing simpler
# works); leftovers of earlier or manual builds go first.
scripts/spotlight-hide-bundles.sh
trap 'scripts/spotlight-hide-bundles.sh || true' EXIT

# ---- phase 1: no credentials in the environment ----
npm run tauri build -- --target "$TARGET" --no-bundle

# ---- phase 2: credentials, bundle + sign + notarize only ----
if [ -f .env.apple ]; then
  set -a; . ./.env.apple; set +a
fi
# The updater signing key (see .env.updater.example): the bundler writes
# Socorin.app.tar.gz plus its .sig, which the in-app updater installs.
if [ -f .env.updater ]; then
  set -a; . ./.env.updater; set +a
fi
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "warning: TAURI_SIGNING_PRIVATE_KEY is not set (.env.updater); the in-app updater cannot install this build." >&2
else
  TAURI_SIGNING_PRIVATE_KEY="${TAURI_SIGNING_PRIVATE_KEY/#\~/$HOME}"
  export TAURI_SIGNING_PRIVATE_KEY
fi

# The Tauri CLI would hand an Apple ID password to notarytool on its command
# line, where every local process can read it. An Apple ID is only used
# through a notarytool keychain profile, never as APPLE_ID / APPLE_PASSWORD.
if [ -n "${APPLE_ID:-}" ] || [ -n "${APPLE_PASSWORD:-}" ]; then
  echo "error: APPLE_ID / APPLE_PASSWORD are not supported (the password would appear on the notarytool command line)." >&2
  echo "       Store them in the keychain once:  xcrun notarytool store-credentials <name> --apple-id <email> --team-id <TEAMID>" >&2
  echo "       then set APPLE_KEYCHAIN_PROFILE=<name>, or use an App Store Connect API key (see .env.apple.example)." >&2
  exit 1
fi

if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "warning: APPLE_SIGNING_IDENTITY is not set; building an ad-hoc signed app (Gatekeeper will warn)." >&2
else
  if ! security find-identity -v -p codesigning | grep -Fq "$APPLE_SIGNING_IDENTITY"; then
    echo "error: no valid identity \"$APPLE_SIGNING_IDENTITY\" in the keychain. Valid identities:" >&2
    security find-identity -v -p codesigning >&2
    exit 1
  fi
  case "$APPLE_SIGNING_IDENTITY" in
    "Developer ID Application:"*) ;;
    *) echo "warning: only a \"Developer ID Application\" identity avoids Gatekeeper warnings outside the App Store." >&2 ;;
  esac
fi

NOTARIZE=0
NOTARY_ARGS=()
if [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ] && [ -n "${APPLE_API_KEY_PATH:-}" ]; then
  APPLE_API_KEY_PATH="${APPLE_API_KEY_PATH/#\~/$HOME}"
  export APPLE_API_KEY_PATH
  [ -f "$APPLE_API_KEY_PATH" ] || { echo "error: APPLE_API_KEY_PATH not found: $APPLE_API_KEY_PATH" >&2; exit 1; }
  NOTARY_ARGS=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
  NOTARIZE=1
elif [ -n "${APPLE_KEYCHAIN_PROFILE:-}" ]; then
  # Tauri cannot use a keychain profile, so it only signs; the script
  # notarizes the .dmg below, which covers the app inside it.
  NOTARY_ARGS=(--keychain-profile "$APPLE_KEYCHAIN_PROFILE")
  NOTARIZE=1
elif [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "warning: no notarization credentials; the app will be signed but not notarized (Gatekeeper will warn)." >&2
fi

# Tauri signs the .app (hardened runtime), notarizes and staples it when the
# variables above are set, then signs the .dmg. The CLI is called directly:
# no npm lifecycle scripts run with the credentials loaded either.
node_modules/.bin/tauri bundle --target "$TARGET" --bundles app,dmg

BUNDLE="src-tauri/target/$TARGET/release/bundle"
APP=$(ls -d "$BUNDLE"/macos/*.app | head -1)
DMG=$(ls "$BUNDLE"/dmg/*.dmg | head -1)

if [ "$NOTARIZE" = 1 ]; then
  # Notarize the disk image as well, so Finder accepts it before the app is
  # even copied out (Apple recommends notarizing the container too). With a
  # keychain profile this is the only submission and it notarizes the app
  # inside the image as well.
  echo "Notarizing $(basename "$DMG") ..."
  SUBMIT_OUT=$(xcrun notarytool submit "$DMG" "${NOTARY_ARGS[@]}" --wait 2>&1 | tee /dev/stderr)
  if ! grep -q "status: Accepted" <<<"$SUBMIT_OUT"; then
    SUBMIT_ID=$(sed -nE 's/^ *id: ([0-9a-f-]{36}).*/\1/p' <<<"$SUBMIT_OUT" | head -1)
    [ -n "$SUBMIT_ID" ] && xcrun notarytool log "$SUBMIT_ID" "${NOTARY_ARGS[@]}" >&2 || true
    echo "error: notarization was not accepted" >&2
    exit 1
  fi
  xcrun stapler staple "$DMG"
  # The ticket also covers the .app's code hash, so the build-directory copy
  # can be stapled too (a no-op when Tauri already did). The copy inside the
  # .dmg carries no staple; Gatekeeper validates it online on first launch.
  xcrun stapler staple "$APP" >/dev/null 2>&1 || true
fi

echo "--- verification ---"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -dvv "$APP" 2>&1 | grep -E "^(Authority|TeamIdentifier|Timestamp)" || true
# Under the hardened runtime the microphone needs an entitlement
# (src-tauri/Entitlements.plist, named in tauri.conf.json): without it macOS
# refuses the microphone without even asking, for the app and for the
# `screencapture` it starts, and every recording would be silent.
if ! codesign -d --entitlements - "$APP" 2>/dev/null | grep -q "com.apple.security.device.audio-input"; then
  echo "error: $APP lacks the com.apple.security.device.audio-input entitlement; recordings would have no sound." >&2
  exit 1
fi
# Expect "accepted source=Notarized Developer ID"; ad-hoc builds are rejected here.
spctl --assess --type execute --verbose=2 "$APP" || true
if [ "$NOTARIZE" = 1 ]; then
  xcrun stapler validate "$APP" || echo "warning: the .app in the bundle directory has no staple; the .dmg is stapled and Gatekeeper validates the app online" >&2
  xcrun stapler validate "$DMG"
fi

mkdir -p installers
cp "$DMG" installers/
shasum -a 256 "installers/$(basename "$DMG")" | tee "installers/$(basename "$DMG").sha256"
echo "Installer: installers/$(basename "$DMG")"

# The updater artifact (the signed .app as a tarball) and its signature,
# named like the disk image so scripts/publish-landing.sh finds them.
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
UPDATER="$BUNDLE/macos/Socorin.app.tar.gz"
if [ -f "$UPDATER" ] && [ -f "$UPDATER.sig" ]; then
  cp "$UPDATER" "installers/Socorin_${VERSION}_${ARCH}.app.tar.gz"
  cp "$UPDATER.sig" "installers/Socorin_${VERSION}_${ARCH}.app.tar.gz.sig"
  echo "Updater artifact: installers/Socorin_${VERSION}_${ARCH}.app.tar.gz (+ .sig)"
else
  echo "warning: no updater artifact was produced (TAURI_SIGNING_PRIVATE_KEY missing?)" >&2
fi
