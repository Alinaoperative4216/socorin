#!/usr/bin/env bash
# Moves the Socorin.app bundles that macOS builds leave in the cargo target
# directory into a folder Spotlight skips:
#   …/release/bundle/macos/Socorin.app -> …/release/bundle/macos.noindex/Socorin.app
#
# Spotlight lists every Socorin.app it finds, so each bundle under
# src-tauri/target/*/release/bundle/macos/ showed up next to the installed
# app (and deleting the wrong one in Finder removes the real app). A marker
# file (.metadata_never_index) does not help: macOS honours it at the root
# of a volume only. What Spotlight does skip is a folder whose name ends in
# ".noindex" (Xcode's build folders rely on the same rule).
#
# The whole target directory cannot be hidden that way behind a symlink
# (target -> target.noindex): on macOS Tauri refuses to work from an
# executable whose path contains a symlink (tauri_utils' StartingBinary),
# which breaks the updater, its tests and `tauri dev`. So only the finished
# bundles move; they stay available to install or inspect by hand.
#
# scripts/build-macos.sh runs this before it builds and when it ends (also
# when it fails). Run it by hand after a manual `tauri build` / `tauri
# bundle` on a Mac. Safe to run any number of times.
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET_DIR="${CARGO_TARGET_DIR:-src-tauri/target}"
[ -d "$TARGET_DIR" ] || exit 0

find "$TARGET_DIR" -maxdepth 5 -type d -path "*/bundle/macos/*.app" -prune -print0 |
  while IFS= read -r -d '' app; do
    dest="$(dirname "$(dirname "$app")")/macos.noindex"
    mkdir -p "$dest"
    rm -rf "${dest:?}/$(basename "$app")"
    mv "$app" "$dest/"
    echo "hidden from Spotlight: $dest/$(basename "$app")"
  done
