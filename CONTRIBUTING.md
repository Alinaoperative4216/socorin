# Contributing to Socorin

Thanks for helping. This page covers what a change needs before it can be
merged; the [README](README.md) explains how to build and run the app.

## Before you start

- Open an issue for anything bigger than a small fix, so the approach can be
  agreed first. Security problems go through [SECURITY.md](SECURITY.md),
  not the issue tracker.
- Product UI strings are English.

## Setup

Prerequisites and the `npm run tauri dev` loop are in the README under
*Development*. `src/devmock.ts` lets you drive the overlay, editor and
settings in a plain browser (`http://localhost:1420/?mock=overlay-1`)
without the Tauri runtime.

## Tests and coverage (required)

Every change ships with unit tests, and line coverage must stay at or above
75 % on both halves of the code base. A pull request that drops either
number below that is not ready.

| Half | Runner | Gate |
|---|---|---|
| Frontend (`src/**`) | Vitest + jsdom + Testing Library | `npm run test:coverage` (fails below 75 % lines, see `vitest.config.ts`) |
| Rust (`src-tauri/src/**`) | `cargo test --lib` | `npm run test:rust:coverage` (`cargo llvm-cov --fail-under-lines 75`) |

`npm run test:all` runs both. The Rust gate needs
`rustup component add llvm-tools-preview` and `cargo install cargo-llvm-cov`.
CI runs the same commands on every push and pull request.

### How the tests are set up

Frontend

- Tests live next to the code as `*.test.ts` / `*.test.tsx`; `src/test/`
  holds the shared harness and is excluded from coverage.
- `src/test/setup.ts` stubs what jsdom lacks (2D canvas context,
  `ImageData`, `ResizeObserver`, image decoding, `toBlob`, object URLs).
  Konva renders for real on top of these stubs, so the stage and shape
  components are tested by mounting them.
- `src/test/tauri.ts` (`installTauri(label, handlers)`) installs the same
  `window.__TAURI_INTERNALS__` hook the real `@tauri-apps/api` uses, so
  components run unmodified: commands are answered by `handlers`,
  `tauri.calls(cmd)` lists what was invoked, `tauri.emit(event, payload)`
  delivers Rust events.
- `isMac` is decided at import time; use
  `mod = isMac ? { metaKey } : { ctrlKey }` for shortcuts. Mount with real
  timers first, then `vi.useFakeTimers()` if needed (Testing Library's
  `findBy*` does not advance Vitest fake timers).

Rust

- Tests are `#[cfg(test)] mod …` blocks in each file.
- `src-tauri/src/test_support.rs` builds the app on Tauri's mock runtime
  through the same `configure()` the real app uses (plugins, state,
  commands). All app-taking functions are generic over `R: tauri::Runtime`
  for that reason; keep new ones generic (`app: &AppHandle<R>`), never
  `AppHandle` alone.
- One mock app is shared by every test behind a lock
  (`test_support::app()` / `app_with(settings)`): the global-shortcut plugin
  can exist only once per process. Windows created by earlier tests stay,
  so assert on presence, not on "was created". `HOME` is redirected to a
  temp directory for the whole test process; nothing the tests do may touch
  the real home.
- Never call, from a test on a developer machine: `ensure_permission` /
  `begin_session` / `begin_fullscreen` with an idle session (macOS
  permission prompt), `capture_monitors` (screen grab), `focus_overlay` /
  `begin_annotation` / `show_welcome` / `welcome_ready` (steals focus),
  `copy_png` with a valid PNG (overwrites the clipboard), `save_png_as`,
  `open_save_dir`, `open_screen_permission_settings`, `record::start` with
  a valid area, `wgc::Recorder::start` (Windows: records the screen),
  `record::stop` on a non-empty file (opens Finder), `tray::create`,
  `quit`. Use the guarded paths instead (a busy session, an injected
  `record::Active`, an unknown monitor).

## Pull requests

- One topic per pull request, with an imperative summary line ("Sanitise
  the file-name prefix") and a body that says what changed and why.
- Say how you tested it and quote the two coverage percentages from
  `npm run test:all`.
- Measure overlay latency on release builds only; dev-build timings mislead.
- Releases are cut by the maintainers, see
  [docs/RELEASING.md](docs/RELEASING.md).

## License

Socorin is licensed under the Apache License 2.0 (see [LICENSE](LICENSE)).
By submitting a contribution you agree that it is licensed under the same
terms, as section 5 of the license provides.
