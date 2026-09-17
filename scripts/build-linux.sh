#!/usr/bin/env bash
# Builds the Linux bundles in Docker and copies them to installers/.
#   scripts/build-linux.sh          # x86_64 (default)
#   scripts/build-linux.sh arm64    # aarch64
set -euo pipefail
ARCH="${1:-amd64}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="screenshot-linux-builder:$ARCH"
BUNDLES="${BUNDLES:-deb,appimage}"

# Pinned tools for the AppImage fallback (see appimage_fallback below). The
# linuxdeploy assets are Tauri's own builds; update the hashes together
# with the URLs when moving to a newer release.
case "$ARCH" in
  amd64) LINUXDEPLOY_FILE=linuxdeploy-x86_64.AppImage
         LINUXDEPLOY_SHA=e762bea85c8eb0d4b3508d46e5c1f037f717d0f9303ae3b4aafc8b04991fa1ef ;;
  arm64) LINUXDEPLOY_FILE=linuxdeploy-aarch64.AppImage
         LINUXDEPLOY_SHA=b12b5cc57bd0921e1f98d73f58aa364503bc1a27f54b7a69fd2870bce7fa2f55 ;;
  *) echo "unknown arch: $ARCH (amd64 | arm64)" >&2; exit 2 ;;
esac
GTK_PLUGIN_COMMIT=b5eb8d05b4c0ed40107fe2158c5d8527f94568ef
GTK_PLUGIN_SHA=cb379f9b0733e9ad9f8bd78f8c2fa038aef2478523bb7d4c8e64ff6a1ea3501a
export ARCH BUNDLES LINUXDEPLOY_FILE LINUXDEPLOY_SHA GTK_PLUGIN_COMMIT GTK_PLUGIN_SHA

# The updater signing key (see .env.updater.example) goes into the
# container by value: the bundler (or the fallback below) writes a .sig next
# to the AppImage, which the in-app updater checks before it swaps itself.
if [ -f "$ROOT/.env.updater" ]; then
  set -a; . "$ROOT/.env.updater"; set +a
fi
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "warning: TAURI_SIGNING_PRIVATE_KEY is not set (.env.updater); the in-app updater cannot install this build." >&2
else
  TAURI_SIGNING_PRIVATE_KEY="${TAURI_SIGNING_PRIVATE_KEY/#\~/$HOME}"
  if [ -f "$TAURI_SIGNING_PRIVATE_KEY" ]; then
    TAURI_SIGNING_PRIVATE_KEY="$(cat "$TAURI_SIGNING_PRIVATE_KEY")"
  fi
  export TAURI_SIGNING_PRIVATE_KEY
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

# The image is rebuilt from the pinned Dockerfile every time (cheap when
# nothing changed). When the registry cannot be reached for the base
# image's metadata, an image built earlier from the same Dockerfile is used.
if ! docker build --platform "linux/$ARCH" -t "$IMAGE" -f "$ROOT/docker/Dockerfile.linux" "$ROOT/docker"; then
  if docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "warning: docker build failed (registry unreachable?); using the existing $IMAGE" >&2
  else
    exit 1
  fi
fi
mkdir -p "$ROOT/installers"
docker run --rm --platform "linux/$ARCH" \
  -v "$ROOT":/src:ro \
  -v "$ROOT/installers":/out \
  -v "screenshot-cargo-$ARCH":/usr/local/cargo/registry \
  -v "screenshot-target-$ARCH":/build/target \
  -e CARGO_TARGET_DIR=/build/target \
  -e ARCH -e BUNDLES -e LINUXDEPLOY_FILE -e LINUXDEPLOY_SHA -e GTK_PLUGIN_COMMIT -e GTK_PLUGIN_SHA \
  -e TAURI_SIGNING_PRIVATE_KEY -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
  "$IMAGE" bash -euo pipefail -c '
    rsync -a --exclude node_modules --exclude dist --exclude installers \
      --exclude src-tauri/target --exclude .git /src/ /build/app/
    cd /build/app
    npm ci --no-audit --no-fund
    # tauri-bundler downloads linuxdeploy and its plugins to ~/.cache/tauri
    # and zeroes the AppImage magic bytes of linuxdeploy so it runs under the
    # Rosetta / QEMU binfmt rule (which does not match AppImages), but not of
    # linuxdeploy-plugin-appimage.AppImage. linuxdeploy queries every
    # linuxdeploy-plugin-* on PATH at start-up, that one cannot be executed,
    # and it aborts with "subprocess failed (exit code 2)" after the AppDir
    # is prepared. Let it fail, patch every AppImage in the cache and finish
    # the AppImage ourselves (appimage_fallback).
    # The target volume persists between builds: drop the previous bundle
    # output so a renamed product or an old .deb never leaks into this one.
    rm -rf /build/target/release/bundle
    npm run tauri build -- --bundles "$BUNDLES" || echo "bundler failed, trying the AppImage fallback"
    # Download a file and refuse it unless its SHA-256 is the expected one.
    fetch_pinned() {
      local out="$1" url="$2" sha="$3"
      curl -fsSL -o "$out.tmp" "$url"
      echo "$sha  $out.tmp" | sha256sum -c - >/dev/null || {
        echo "checksum mismatch for $url (update scripts/build-linux.sh if the upstream file changed on purpose)" >&2
        rm -f "$out.tmp"; return 1
      }
      mv "$out.tmp" "$out"
    }
    appimage_fallback() {
      local appdir=/build/target/release/bundle/appimage/Socorin.AppDir
      [ -d "$appdir" ] || return 0
      ls /build/target/release/bundle/appimage/*.AppImage >/dev/null 2>&1 && return 0
      local d=/root/.cache/tauri; mkdir -p "$d"; cd "$d"
      [ -f "$LINUXDEPLOY_FILE" ] || fetch_pinned "$LINUXDEPLOY_FILE" \
        "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/$LINUXDEPLOY_FILE" "$LINUXDEPLOY_SHA"
      [ -f linuxdeploy-plugin-gtk.sh ] || fetch_pinned linuxdeploy-plugin-gtk.sh \
        "https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gtk/$GTK_PLUGIN_COMMIT/linuxdeploy-plugin-gtk.sh" "$GTK_PLUGIN_SHA"
      # Zero the "AI\x02" magic so the kernel treats them as plain ELF files.
      for f in "$d"/*.AppImage; do
        printf "\x00\x00\x00" | dd of="$f" bs=1 seek=8 count=3 conv=notrunc 2>/dev/null
      done
      chmod +x "$d"/*
      cd /build/target/release/bundle/appimage
      PATH="$d:$PATH" "$d/$LINUXDEPLOY_FILE" --appimage-extract-and-run \
        --appdir Socorin.AppDir --plugin gtk --output appimage
      local version; version=$(node -p "require(\"/build/app/package.json\").version")
      mv -v Socorin-*.AppImage "Socorin_${version}_${ARCH}.AppImage"
      # The bundler did not get this far, so sign the AppImage for the
      # in-app updater ourselves (same key, same .sig format).
      if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
        (cd /build/app && node_modules/.bin/tauri signer sign \
          "/build/target/release/bundle/appimage/Socorin_${version}_${ARCH}.AppImage")
      fi
    }
    case "$BUNDLES" in *appimage*) appimage_fallback ;; esac
    find /build/target/release/bundle -type f \( -name "*.deb" -o -name "*.AppImage" -o -name "*.rpm" -o -name "*.AppImage.sig" \) -exec cp -v {} /out/ \;
    # Runtime packages the binary links against (to keep deb "depends" honest).
    for p in $(ldd /build/target/release/socorin | awk "/=>/ {print \$3}"); do
      dpkg -S "$p" 2>/dev/null || dpkg -S "/usr$p" 2>/dev/null || true
    done | cut -d: -f1 | sort -u > /out/linux-runtime-packages.txt
  '
cd "$ROOT/installers" && shasum -a 256 *.deb *.AppImage 2>/dev/null | tee SHA256SUMS-linux.txt
ls -la "$ROOT/installers"
