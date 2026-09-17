# Socorin

*English · [Tiếng Việt](README.vi.md)*

[![CI](https://github.com/huystr/socorin/actions/workflows/ci.yml/badge.svg)](https://github.com/huystr/socorin/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Lightweight cross-platform screenshot and annotation tool (macOS, Windows, Linux).
Lives in the menu bar / system tray, wakes up on a global shortcut, lets you drag
a region on any monitor and annotate it with arrows, rectangles, ellipses, lines,
freehand strokes, text, highlights, numbered markers and pixelation.

Built with [Tauri v2](https://tauri.app) (Rust core) and React + TypeScript
(UI, [Konva](https://konvajs.org) for the annotation canvas).

## Download

Installers for macOS (universal `.dmg`), Windows (x64 `.exe`) and Linux
(`.deb`, `.AppImage`) are on <https://socorin.com>, with SHA-256 checksums
at `https://socorin.com/downloads/SHA256SUMS.txt`. Installed copies check
that site for a newer version once a day.

## Features

- Global shortcut (default `Cmd/Ctrl+Shift+A`) for region capture, optional
  second shortcut for full-screen capture. Both configurable.
- Multi-monitor, HiDPI aware region selection overlay.
- Annotate in place: the selected region stays exactly where it is on screen
  (move / resize it until you draw), a toolbar floats next to it and the tools
  draw right on top of the frozen screenshot. Enter or double-click copies and
  finishes, Esc cancels.
- Annotation tools: select/move/resize, rectangle, ellipse, arrow, line, pen,
  text, highlight, numbered marker, pixelate; colour + stroke presets; undo/redo.
  A separate zoomable editor window is one click (or `Ctrl/⌘+E`) away.
- Copy to clipboard, quick save to a folder with timestamped names, or save as.
- *Upload & copy link*: one click (or `Ctrl/⌘+Shift+U`) in the annotation
  toolbar sends the picture, annotations included, to socorin.com and puts a
  share link on the clipboard, announced in a small popover next to the
  button you clicked (Copy again / Delete from server) that also says when
  the file goes. Files are kept for 60 days at most and may be 5 MB at most
  on socorin.com; Socorin itself never uploads
  more than 25 MB, whatever a server allows. The link it copies always has
  to be one on the server the capture went to — an answer pointing anywhere
  else is refused and nothing reaches the clipboard. *Settings → Share*
  lists the links you made, with *Copy* and *Delete* for each (links whose
  time is up drop off the list by themselves), and takes another upload
  server (a self-hosted one, or `http://localhost:3000` while developing).
- "After capture" modes: annotate on screen, copy immediately, save
  immediately, upload and copy the link immediately.
- Screen recording of a region or the full screen (tray menu, hotkey, CLI).
  A region recording keeps a floating bar exactly where the Record / Cancel
  bar was, with the running time, *Stop & copy* (the video goes to the
  clipboard as a file, ready to paste into a chat), *Stop & upload* (the
  video goes to socorin.com and its link to the clipboard, same limits as
  for pictures), *Stop* (reveals the file) and *Cancel* (discards it). A
  full-screen recording shows no bar: the tray icon turns red with the
  running time next to it, and its menu offers the same four actions.
  macOS records with `screencapture`, Windows with the built-in recorder
  (Windows Graphics Capture + Media Foundation, nothing to install), Linux
  with `ffmpeg`. macOS writes QuickTime `.mov` files, which the share server
  does not take: with an `ffmpeg` installed (`brew install ffmpeg`, or a
  path under *Settings → Recording*) *Stop & upload* converts the recording
  to `.mp4` first (no re-encoding); without one it keeps the `.mov` and says
  so.
- Launch at login (on by default; switch it off in Settings), single
  instance, settings window hidden in the tray.
- Update check once a day against `https://socorin.com/version.json` (on by
  default; switch it off under *Updates* in Settings). A newer version is
  announced in a small popover under the menu bar / tray icon (Update now /
  Later), as *Update to Socorin x.y.z…* at the top of the tray menu and in
  Settings, which also has *Check now*. *Install updates automatically* (on
  by default; switch it off under *Updates*) downloads and installs it as
  soon as it is found and restarts the app. Installs go through Tauri's
  updater: the manifest names a signed installer per platform (macOS
  `.app.tar.gz`, Windows NSIS `.exe`, Linux
  `.AppImage`; `.deb` users are sent to the download page) and the app only
  accepts files whose signature matches the public key built into it.
- CLI flags for desktop-environment shortcuts: `--capture`, `--capture-full`,
  `--record`, `--record-full`, `--stop-record`, `--cancel`. The capture and
  record flags are ignored until *Allow command-line triggers* is switched on
  in Settings (any local program could otherwise use the app, which holds the
  screen-recording permission, to take screenshots for it).

## Privacy

Nothing leaves your computer unless you ask for it. Screenshots and
recordings are files on your disk and in your clipboard; the only network
traffic is the daily version check (a `GET` of `version.json`, which can be
switched off) and the uploads you start yourself with *Upload & copy link*,
*Stop & upload* or the *upload* after-capture mode. An upload sends the
file, its type and size, a checksum and the app's version and platform to
the server in *Settings → Share*, plus a random install ID the server hands
out on first use so it can hold off spam: there are no accounts, and *Reset
install ID* discards it. Every link comes with a delete token the app keeps
locally, so you can take a file down again from the popover or from
*Settings → Share* before it expires.

## Development

Prerequisites:

- Node.js 20+ and npm
- Rust stable (`curl https://sh.rustup.rs -sSf | sh`)
- Platform toolchain, see <https://tauri.app/start/prerequisites/>
  - macOS: Xcode Command Line Tools
  - Windows: Visual Studio Build Tools (C++), WebView2 (preinstalled on Win 11)
  - Ubuntu 22.04+:
    `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libpipewire-0.3-dev libdbus-1-dev libxcb1-dev libxcb-randr0-dev`

```bash
npm install
npm run tauri dev
```

Build the installer for the machine you are on with:

```bash
npm run tauri build
```

Cross-building from a Mac (outputs land in `installers/`):

```bash
scripts/build-macos.sh                                    # macOS universal .dmg, signed + notarized when .env.apple exists
scripts/build-windows.sh                                  # Windows x64 .exe (NSIS) via cargo-xwin
scripts/build-linux.sh                                    # Ubuntu x86_64 .deb + .AppImage via Docker
scripts/build-linux.sh arm64                              # Ubuntu aarch64
```

The Windows and Linux builds produced this way are compiled but not
smoke-tested here; run them once on a real machine before shipping. The
Linux script writes `SHA256SUMS-linux.txt` next to the bundles; publish the
checksums with the installers. The Docker images pin Node and rustup by
version and SHA-256, and the AppImage fallback pins its `linuxdeploy`
downloads the same way; `scripts/makensis-docker` mounts only the project
and Tauri's cache into the NSIS container. That container builds the
official NSIS release from its pinned source and stubs instead of installing
Ubuntu's `nsis` package: the distribution's MinGW-built installer stubs end
up with a stale relocation directory that PE parsers and antivirus engines
report as corrupt.

### Signing and notarizing the macOS build

A plain build is ad-hoc signed: on another Mac, Gatekeeper reports that the
app cannot be verified and the user has to allow it in *System Settings →
Privacy & Security*. To ship without warnings the app must be signed with a
**Developer ID Application** certificate (paid Apple Developer Program) and
notarized by Apple. `scripts/build-macos.sh` does both through the Tauri CLI:

1. **Certificate.** Keychain Access → *Certificate Assistant → Request a
   Certificate From a Certificate Authority…* (saved to disk). At
   [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates)
   create a *Developer ID Application* certificate from that request, download
   the `.cer` and double-click it. `security find-identity -v -p codesigning`
   must now list `Developer ID Application: <name> (<TEAMID>)` as valid (if it
   is not trusted, install Apple's *Developer ID G2* intermediate from
   [apple.com/certificateauthority](https://www.apple.com/certificateauthority/)).
2. **Notarization credentials.** Either an App Store Connect API key
   (*Users and Access → Integrations → App Store Connect API → Team Keys*,
   role Developer; download the `.p8` and keep it outside the repository),
   or an Apple ID with an app-specific password stored in the keychain once
   with `xcrun notarytool store-credentials <name> --apple-id <email> --team-id <TEAMID>`
   (type the password at the prompt) and referenced as
   `APPLE_KEYCHAIN_PROFILE=<name>`. `APPLE_ID` / `APPLE_PASSWORD` in the
   environment are refused: the Tauri CLI would put that password on the
   `notarytool` command line, where any local process can read it.
3. Copy `.env.apple.example` to `.env.apple` (git-ignored) and fill it in.
4. `scripts/build-macos.sh` (or `aarch64` / `x86_64` for a single
   architecture). The frontend and cargo build run first without the
   credentials; only the Tauri CLI's bundling step sees them. Tauri signs
   the app with the hardened runtime, submits it to Apple, staples the
   ticket and signs the `.dmg`; the script then notarizes and staples the
   `.dmg` too, prints `spctl` / `stapler` verification (expect
   `accepted source=Notarized Developer ID`) and writes a `.sha256` next to
   the installer. Notarization usually takes one to five minutes.

With a keychain profile Tauri only signs; the script submits the `.dmg`
(which notarizes the app inside it) and staples the `.dmg` and the
build-directory `.app`. The copy inside the `.dmg` carries no staple and is
validated online by Gatekeeper on first launch.

The same `APPLE_*` variables work in CI: export a `.p12` of the certificate as
`APPLE_CERTIFICATE` (base64) with `APPLE_CERTIFICATE_PASSWORD` instead of
relying on the keychain. Signed builds keep a stable identity, so the Screen
Recording permission survives updates.

### Signing the Windows build

Tauri runs `scripts/windows-sign.sh` on the app, the installer, its
uninstaller and the NSIS plugin (`bundle.windows.signCommand`). Without a
certificate it only fills in the PE header checksum that neither lld-link
nor makensis writes; the build stays unsigned, so SmartScreen warns and
machine-learning antivirus engines may still flag it (an unsigned, brand-new
installer with screen-capture, hotkey and clipboard APIs matches their
spyware heuristics). To sign, get an OV or EV code-signing certificate as a
`.pfx`, copy `.env.windows.example` to `.env.windows` (git-ignored), fill in
`WINDOWS_SIGN_PFX` and `WINDOWS_SIGN_PASSWORD`, install `osslsigncode`
(`brew install osslsigncode`) and run `scripts/build-windows.sh` again;
`scripts/makensis-docker` passes the certificate into the container for the
uninstaller. This path is wired up but has not been exercised with a real
certificate yet.

### Signing updates

The in-app updater verifies every download against the public key in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`); the matching private
key signs the updater artifacts at build time (`bundle.createUpdaterArtifacts`).
The key pair was made with `tauri signer generate -w ~/.tauri/socorin.key`
(no password); copy `.env.updater.example` to `.env.updater` (git-ignored)
and point `TAURI_SIGNING_PRIVATE_KEY` at the key. All three build scripts
load it: the macOS script copies `Socorin_<v>_universal.app.tar.gz` and its
`.sig` next to the `.dmg`, the Windows one the installer's `.sig`, the Linux
one the AppImage's `.sig` (signed by the fallback itself when the bundler's
AppImage step did not run). Without the key the builds still work, but
nothing signed is produced and the updater cannot install them.

Losing the private key means no existing install can ever update itself
again; keep it backed up. Cutting a release and publishing it to
socorin.com is described in [docs/RELEASING.md](docs/RELEASING.md).

### Testing without a mouse

Two dev aids exist so the whole pipeline can be exercised automatically:

- **Browser mock** (`src/devmock.ts`): with `npm run dev` running, open
  `http://localhost:1420/?mock=overlay-1`, `?mock=editor`, `?mock=main` or
  `?mock=update` (`&state=installing|failed|updated`) in a normal browser. The Tauri runtime is replaced by an in-page stub with a
  synthetic screenshot, so the overlay, editor and settings UI can be driven
  and inspected with regular browser tooling.
- **End-to-end harness** (`src-tauri/src/debug.rs`), all opt-in via env vars:

  ```bash
  SOCORIN_DEBUG=1 \
  SOCORIN_DEBUG_DIR=/tmp/socorin-dump \
  SOCORIN_DEBUG_AUTOSELECT=200,150,900,600 \
  SOCORIN_DEBUG_AUTOACTION=save \
  npm run tauri dev
  ```

  Then trigger a capture (`target/debug/socorin --capture` from a second
  shell once *Allow command-line triggers* is on, the tray menu or the hotkey).
  The overlay auto-selects the region, the in-place editor adds one of every
  annotation and saves the result to the screenshots folder; captured monitors
  are dumped to `SOCORIN_DEBUG_DIR`.

  Only `SOCORIN_DEBUG` (logging) exists in a release build. The other
  variables are compiled out unless the build is made with
  `npm run tauri build -- --features debug-harness`, so a shipped binary can
  never be steered by its environment.

## Platform notes

- **macOS** needs the Screen Recording permission (System Settings → Privacy &
  Security → Screen Recording). The app prompts the first time; if capture
  returns an empty image after granting, relaunch the app.
- **macOS: install into Applications first.** Started from the mounted `.dmg`
  (or from Gatekeeper's translocated copy) the app cannot be granted Screen
  Recording; macOS often does not even show the prompt and the app never
  appears in the list. The settings window says so. Drag `Socorin.app` to
  Applications, eject the image, start it from there.
- **macOS: ad-hoc signed builds** (no Developer ID, see *Signing and
  notarizing* above) are identified by the hash of the exact binary, so
  after every update the old permission entry goes stale: the switch may still be on while capturing fails, or the app is gone
  from the list. Reset it and grant again:
  `tccutil reset ScreenCapture com.socorin`.
- **Linux**: the `xcap` capture crate binds against pipewire 1.x headers, so
  the Linux build (and the `.deb`/`.AppImage` it produces) targets Ubuntu
  24.04 or newer; 22.04 ships pipewire 0.3.48 and does not compile it.
- **Linux / Wayland** (Ubuntu default since 22.04): compositors do not let apps
  register global shortcuts. Bind a custom shortcut in *Settings → Keyboard*
  to `socorin --capture` instead and turn on *Allow command-line triggers*
  in the app's settings; the running instance picks it up.
  Screen capture goes through the GNOME Shell screenshot D-Bus API or the
  xdg-desktop-portal, which `xcap` handles. On X11 everything works natively.
- **Windows**: no special setup. Per-monitor DPI with mixed scale factors is
  handled by normalising monitor geometry to logical units. Recording uses
  Windows Graphics Capture and a Media Foundation H.264 encoder
  (hardware-accelerated where the GPU offers it) on Windows 10 version 1903
  or later. Windows 10 draws its capture border around the recorded display;
  Windows 11 is asked once for borderless capture. Where the built-in
  recorder is unavailable an installed `ffmpeg` takes over, and a path under
  *Settings → Recording* makes `ffmpeg` the recorder outright.
- **Recording on Linux** uses `ffmpeg`. The app looks for it in the
  usual install locations and then in the absolute entries of `PATH` (never
  the working directory); *Settings → Recording* takes an explicit path when
  it lives elsewhere (it has to be the `ffmpeg` binary itself: the setting
  cannot point the recorder at another program). macOS records with the
  system `screencapture`.

## Contributing

Bug reports and pull requests are welcome; [CONTRIBUTING.md](CONTRIBUTING.md)
has the test and coverage rules every change must meet. Security problems
go through [SECURITY.md](SECURITY.md), not the public issue tracker.

## License

Socorin is released under the [Apache License 2.0](LICENSE). Contributions
are accepted under the same terms (section 5 of the license).
