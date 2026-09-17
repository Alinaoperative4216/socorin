#!/usr/bin/env bash
# Publishes a release to the landing-page checkout: copies the installers
# and the updater artifacts with their signatures into public/downloads/
# (git-ignored there; the Docker image takes them from the working tree),
# and commits the checksums, version.json (the manifest the app reads once
# a day and the updater plugin installs from) and the download links.
#
#   scripts/publish-landing.sh                 # version from tauri.conf.json
#   scripts/publish-landing.sh 1.0.2 "notes"   # explicit version, release notes
#
# Expects in installers/ (from scripts/build-*.sh):
#   Socorin_<v>_universal.dmg, Socorin_<v>_universal.app.tar.gz (+ .sig)
#   Socorin_<v>_x64-setup.exe (+ .sig)
#   Socorin_<v>_amd64.deb, Socorin_<v>_amd64.AppImage (+ .sig)
# A missing updater artifact only drops that platform from the manifest
# (its users are sent to the download page); a missing installer aborts.
# LANDING_DIR overrides the landing-page checkout (default: sibling folder).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LANDING="${LANDING_DIR:-$ROOT/../socorin-landing-page}"
VERSION="${1:-$(node -p "require('$ROOT/src-tauri/tauri.conf.json').version")}"
NOTES="${2:-}"
SRC="$ROOT/installers"
DL="$LANDING/public/downloads"
[ -d "$DL" ] || { echo "no landing page at $LANDING (set LANDING_DIR)" >&2; exit 1; }

INSTALLERS=(
  "Socorin_${VERSION}_universal.dmg"
  "Socorin_${VERSION}_x64-setup.exe"
  "Socorin_${VERSION}_amd64.deb"
  "Socorin_${VERSION}_amd64.AppImage"
)
for f in "${INSTALLERS[@]}"; do
  [ -f "$SRC/$f" ] || { echo "missing installer: $SRC/$f" >&2; exit 1; }
done
UPDATER=("Socorin_${VERSION}_universal.app.tar.gz" "Socorin_${VERSION}_x64-setup.exe" "Socorin_${VERSION}_amd64.AppImage")

# Out with the previous release's files (whatever product name they had).
find "$DL" -maxdepth 1 -type f \( -name "Socorin_*" -o -name "Screenshot_*" \) ! -name "*_${VERSION}_*" -print -delete
for f in "${INSTALLERS[@]}"; do cp -v "$SRC/$f" "$DL/"; done
# Always copied, even when a file of that name is already there: a rebuild
# of the same version (notarized this time, say) changes the bytes, and the
# signature must belong to the file next to it.
for f in "${UPDATER[@]}"; do
  if [ -f "$SRC/$f.sig" ]; then
    cp -v "$SRC/$f" "$DL/"
    cp -v "$SRC/$f.sig" "$DL/"
  else
    echo "warning: no signature for $f; that platform gets no automatic update" >&2
  fi
done

# Checksums with bare file names (run inside the folder).
(cd "$DL" && shasum -a 256 Socorin_"${VERSION}"_* | grep -v '\.sig$' > SHA256SUMS.txt && cat SHA256SUMS.txt)

# version.json: `version` for the daily check, `platforms` for the updater
# plugin, which looks up "<os>-<arch>[-<installer>]" (see
# src-tauri/src/update.rs). Only .deb has no entry: its users are sent to
# the download page.
BASE="https://socorin.com/downloads"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
VERSION="$VERSION" NOTES="$NOTES" BASE="$BASE" PUB_DATE="$PUB_DATE" DL="$DL" node - <<'EOF'
const fs = require("fs");
const path = require("path");
const { VERSION, NOTES, BASE, PUB_DATE, DL } = process.env;
const sig = (name) => {
  const p = path.join(DL, `${name}.sig`);
  return fs.existsSync(p) ? fs.readFileSync(p, "utf8").trim() : null;
};
const entry = (name) => {
  const signature = sig(name);
  return signature ? { url: `${BASE}/${name}`, signature } : null;
};
const mac = entry(`Socorin_${VERSION}_universal.app.tar.gz`);
const win = entry(`Socorin_${VERSION}_x64-setup.exe`);
const linux = entry(`Socorin_${VERSION}_amd64.AppImage`);
const platforms = {};
if (mac) {
  platforms["darwin-aarch64"] = mac;
  platforms["darwin-x86_64"] = mac;
}
if (win) platforms["windows-x86_64"] = win;
if (linux) platforms["linux-x86_64-appimage"] = linux;
const manifest = { version: VERSION, notes: NOTES || `Socorin ${VERSION}`, pub_date: PUB_DATE, platforms };
fs.writeFileSync(path.join(DL, "..", "version.json"), JSON.stringify(manifest, null, 2) + "\n");
console.log(`version.json: ${VERSION}, platforms: ${Object.keys(platforms).join(", ") || "none"}`);
EOF

# The download links and the version label in the page.
PAGE="$LANDING/components/landing-page.tsx"
sed -E -i '' \
  -e "s#/downloads/Socorin_[0-9]+\.[0-9]+\.[0-9]+_#/downloads/Socorin_${VERSION}_#g" \
  -e "s#>v[0-9]+\.[0-9]+\.[0-9]+<#>v${VERSION}<#g" \
  "$PAGE"
grep -c "Socorin_${VERSION}_" "$PAGE" >/dev/null || { echo "no download links updated in $PAGE" >&2; exit 1; }

cd "$LANDING"
# The installers themselves stay out of git (see .gitignore there).
git add public/downloads/SHA256SUMS.txt public/version.json components/landing-page.tsx
git commit -m "Socorin ${VERSION}: installers, updater manifest, checksums and links" \
  -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
echo "Committed in $LANDING; redeploy the landing page so socorin.com/version.json serves ${VERSION}."
