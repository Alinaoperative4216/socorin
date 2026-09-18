# Releasing Socorin

Maintainer notes. Users get the app from <https://socorin.com>; installed
copies read `https://socorin.com/version.json` once a day and install
signed updates from there.

## 1. Bump the version

The version lives in three files that must agree: `package.json`,
`src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` (the daily check
compares the manifest's `version` with the one in `tauri.conf.json`).
`cargo build` refreshes `src-tauri/Cargo.lock`. Commit the bump.

## 2. Build the installers

From a clean checkout on a Mac with Docker running (prerequisites and the
signing setup are in the README under *Development*):

```bash
scripts/build-macos.sh      # universal .dmg, signed + notarized when .env.apple exists
scripts/build-windows.sh    # x64 NSIS .exe via cargo-xwin, signed when .env.windows exists
scripts/build-linux.sh      # x86_64 .deb + .AppImage via Docker
```

Everything lands in `installers/`. With `.env.updater` present each script
also writes the updater artifact and its signature next to the installer:
`Socorin_<v>_universal.app.tar.gz` + `.sig`, `Socorin_<v>_x64-setup.exe.sig`
and `Socorin_<v>_amd64.AppImage.sig`. Without the key the builds still work,
but the in-app updater cannot install them.

Run the Windows and Linux installers once on a real machine before
publishing: the cross-builds are known to work there, but the Mac that
compiles them cannot start them.

## 3. Publish

`scripts/publish-landing.sh [version] [notes]` puts the release on the
landing page, whose checkout is expected next to this one as
`../socorin-landing-page` (override with `LANDING_DIR`). In one commit
there it:

1. copies the installers and the updater artifacts with their signatures to
   `public/downloads/` and removes the previous version's files (the
   installers are git-ignored in that repository and reach production
   through the Docker image built from the working tree);
2. regenerates `public/downloads/SHA256SUMS.txt`;
3. writes `public/version.json`, the manifest the app reads:
   `{ "version", "notes", "pub_date", "platforms": { "darwin-aarch64",
   "darwin-x86_64", "windows-x86_64", "linux-x86_64-appimage": { "url",
   "signature" } } }`. A platform without a signature is left out and its
   users are sent to the download page;
4. rewrites the download links and the version label in
   `components/landing-page.tsx`.

Then redeploy the landing page so `socorin.com/version.json` serves the new
version.

## Keys

- The updater key pair (private half generated with
  `tauri signer generate`, public half in `src-tauri/tauri.conf.json`) is
  what makes installed copies accept an update. Losing the private key
  means no existing install can ever update itself again; keep it backed up.
- The Apple Developer ID certificate and the notarization credentials are
  described in the README (*Signing and notarizing the macOS build*).

## Checking an update end to end

A build made with `--features debug-harness` reads the manifest from
`SOCORIN_DEBUG_UPDATE_URL` instead of socorin.com (see
`src-tauri/src/debug.rs`), so the whole flow can be tried against a local
manifest before anything is published.
