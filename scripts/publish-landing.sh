#!/usr/bin/env bash
# Publishes a release to the landing-page checkout: copies the installers
# and the updater artifacts with their signatures into public/downloads/
# (git-ignored there; the Docker image takes them from the working tree),
# and commits the checksums, version.json (the manifest the app reads once
# a day and the updater plugin installs from), the download links and the
# installer sizes on the page (and their translations in lib/i18n.ts). The
# commit takes only those files: other work in that checkout stays out.
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

# The installer sizes on the page ("Bộ cài .exe · 3,7 MB", "Universal .dmg ·
# 7,6 MB", the sentence naming both, and the big Windows figure that stands
# without its unit, `t("3,7")`) and their English translations in
# lib/i18n.ts: decimal megabytes with one decimal, a comma in Vietnamese, a
# dot in English. The previous sizes are read from the page itself.
I18N="$LANDING/lib/i18n.ts"
mb() { node -p "(require('fs').statSync('$1').size / 1e6).toFixed(1)"; }
NEW_WIN=$(mb "$SRC/Socorin_${VERSION}_x64-setup.exe")
NEW_MAC=$(mb "$SRC/Socorin_${VERSION}_universal.dmg")
OLD_WIN=$(sed -nE 's/.*Bộ cài \.exe · ([0-9]+,[0-9]) MB.*/\1/p' "$PAGE" | head -1 | tr ',' '.')
OLD_MAC=$(sed -nE 's/.*Universal \.dmg · ([0-9]+,[0-9]) MB.*/\1/p' "$PAGE" | head -1 | tr ',' '.')
if [ -z "$OLD_WIN" ] || [ -z "$OLD_MAC" ]; then
  echo "warning: the installer sizes were not found in $PAGE; update them by hand" >&2
elif { [ "$OLD_WIN" = "$OLD_MAC" ] && [ "$NEW_WIN" != "$NEW_MAC" ]; } || [ "$NEW_WIN" = "$OLD_MAC" ] || [ "$NEW_MAC" = "$OLD_WIN" ]; then
  echo "warning: the old and new sizes overlap (Windows $OLD_WIN -> $NEW_WIN, macOS $OLD_MAC -> $NEW_MAC); update them by hand" >&2
else
  for f in "$PAGE" "$I18N"; do
    sed -E -i '' \
      -e "s/${OLD_WIN/./,} MB/${NEW_WIN/./,} MB/g" -e "s/${OLD_WIN/./\\.} MB/${NEW_WIN} MB/g" \
      -e "s/${OLD_MAC/./,} MB/${NEW_MAC/./,} MB/g" -e "s/${OLD_MAC/./\\.} MB/${NEW_MAC} MB/g" \
      -e "s/t\\(\"${OLD_WIN/./,}\"\\)/t(\"${NEW_WIN/./,}\")/g" \
      -e "s/\"${OLD_WIN/./,}\": \"${OLD_WIN/./\\.}\"/\"${NEW_WIN/./,}\": \"${NEW_WIN}\"/g" \
      "$f"
  done
  echo "sizes: Windows ${NEW_WIN} MB, macOS ${NEW_MAC} MB (were ${OLD_WIN} / ${OLD_MAC})"
  # The big figure only follows when it agreed with the rest of the page.
  grep -q "t(\"${NEW_WIN/./,}\")" "$PAGE" && grep -q "\"${NEW_WIN/./,}\": \"${NEW_WIN}\"" "$I18N" ||
    echo "warning: the big Windows size figure (t(\"N,N\") in $PAGE and its line in $I18N) does not say ${NEW_WIN/./,}; update it by hand" >&2
fi

cd "$LANDING"
# The installers themselves stay out of git (see .gitignore there). Only
# the files below go into the commit: other work in that checkout, staged
# or not, stays out of it.
PUBLISHED=(public/downloads/SHA256SUMS.txt public/version.json components/landing-page.tsx lib/i18n.ts)
git add -- "${PUBLISHED[@]}"
git commit -m "Socorin ${VERSION}: installers, updater manifest, checksums, links and sizes" -- "${PUBLISHED[@]}"
echo "Committed in $LANDING; push it and redeploy the landing page so socorin.com/version.json serves ${VERSION}."
