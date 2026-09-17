//! Screen capture session: grabs every monitor (in parallel), keeps the
//! bitmaps in memory while the overlay windows are visible, crops the chosen
//! region and hands the result to the editor / clipboard / disk.
//!
//! Overlay windows are created once and kept hidden between captures (see
//! `windows::ensure_overlays`), so a capture only has to grab the screen,
//! push the pixels to the already-loaded webviews and show them.

use std::{
    fs::OpenOptions,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, EventTarget, Manager, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;
#[cfg(not(target_os = "macos"))]
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use xcap::{
    image::{
        codecs::png::{CompressionType, FilterType, PngEncoder},
        ExtendedColorType, ImageEncoder, RgbaImage,
    },
    Monitor,
};

use crate::{
    cursor, debug,
    settings::{self, AfterCapture},
    windows,
};

/// Escape cancels the overlay. It is registered as a *global* shortcut for
/// the duration of a session so it works even when the OS refused to make
/// the overlay the key window.
const CANCEL_SHORTCUT: &str = "Escape";

#[derive(Debug, Clone, Copy, Serialize)]
struct CursorPos {
    x: f64,
    y: f64,
}

/// What an overlay session is for: pick a region to screenshot, or to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Screenshot,
    Record,
}

/// Monitor placement in *logical* (DPI-independent) units, ready for Tauri's
/// window API.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorGeom {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
}

/// Everything an overlay needs to display one captured monitor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    #[serde(flatten)]
    pub geom: MonitorGeom,
    pub image_width: u32,
    pub image_height: u32,
    /// Increments per capture so an overlay can ignore duplicate start signals.
    pub session: u64,
    /// Full-screen capture in annotate mode: the overlay selects the whole
    /// monitor by itself instead of waiting for a drag.
    pub preselect_full: bool,
    pub mode: Mode,
}

pub struct MonitorShot {
    pub geom: MonitorGeom,
    /// Physical pixels.
    pub image: RgbaImage,
}

/// A region inside a monitor bitmap, in physical pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub monitor_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
pub struct AppState {
    /// Bitmaps of the current capture session (empty when idle).
    pub shots: Mutex<Vec<MonitorShot>>,
    /// PNG waiting to be picked up by the editor window.
    pub pending: Mutex<Option<Vec<u8>>>,
    /// True while a session is active, prevents re-entrancy from the hotkey.
    pub busy: AtomicBool,
    /// A region is selected and being annotated on the overlay.
    pub annotating: AtomicBool,
    /// The session picks an area to record rather than to screenshot.
    pub record_mode: AtomicBool,
    pub session: AtomicU64,
    /// When the current session started (for latency diagnostics).
    pub started: Mutex<Option<Instant>>,
}

fn geometry_of(m: &Monitor) -> Result<MonitorGeom, String> {
    let id = m.id().map_err(|e| e.to_string())?;
    let scale = m.scale_factor().unwrap_or(1.0).max(0.1);
    let (mut x, mut y) = (m.x().unwrap_or(0), m.y().unwrap_or(0));
    let (mut width, mut height) = (
        m.width().map_err(|e| e.to_string())?,
        m.height().map_err(|e| e.to_string())?,
    );
    // xcap reports physical pixels on Windows but logical units on
    // macOS / Linux. Normalise to logical.
    if cfg!(target_os = "windows") {
        x = (x as f32 / scale).round() as i32;
        y = (y as f32 / scale).round() as i32;
        width = (width as f32 / scale).round() as u32;
        height = (height as f32 / scale).round() as u32;
    }
    Ok(MonitorGeom {
        id,
        name: m.friendly_name().or_else(|_| m.name()).unwrap_or_default(),
        x,
        y,
        width,
        height,
        scale,
        primary: m.is_primary().unwrap_or(false),
    })
}

/// Current monitor layout (cheap, no capture).
pub fn monitor_geometry() -> Result<Vec<MonitorGeom>, String> {
    let monitors = Monitor::all().map_err(|e| format!("cannot list monitors: {e}"))?;
    let geoms = monitors
        .iter()
        .map(geometry_of)
        .collect::<Result<Vec<_>, _>>()?;
    if geoms.is_empty() {
        return Err("no monitors found".into());
    }
    Ok(geoms)
}

/// Capture all monitors concurrently. Each thread re-enumerates the monitors
/// itself because xcap's handles are not `Send` on every platform.
fn capture_monitors() -> Result<Vec<MonitorShot>, String> {
    let geoms = monitor_geometry()?;
    let results: Vec<Result<MonitorShot, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = geoms
            .into_iter()
            .map(|geom| {
                scope.spawn(move || -> Result<MonitorShot, String> {
                    let monitors = Monitor::all().map_err(|e| e.to_string())?;
                    let monitor = monitors
                        .into_iter()
                        .find(|m| m.id().ok() == Some(geom.id))
                        .ok_or_else(|| format!("monitor {} disappeared", geom.id))?;
                    let image = monitor
                        .capture_image()
                        .map_err(|e| format!("cannot capture monitor {}: {e}", geom.id))?;
                    Ok(MonitorShot { geom, image })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("capture thread panicked".into())))
            .collect()
    });
    results.into_iter().collect()
}

pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity((img.width() * img.height()) as usize);
    let enc = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive);
    enc.write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("png encode failed: {e}"))?;
    Ok(out)
}

fn crop(img: &RgbaImage, r: Region) -> Result<RgbaImage, String> {
    let x = r.x.min(img.width().saturating_sub(1));
    let y = r.y.min(img.height().saturating_sub(1));
    let w = r.width.min(img.width() - x);
    let h = r.height.min(img.height() - y);
    if w == 0 || h == 0 {
        return Err("empty region".into());
    }
    Ok(xcap::image::imageops::crop_imm(img, x, y, w, h).to_image())
}

#[cfg(target_os = "macos")]
pub(crate) fn ensure_permission<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if let Some(issue) = crate::macos::install_issue() {
        // Asking would be pointless (see `macos::install_issue`): explain instead.
        windows::show_settings(app);
        return Err(crate::macos::install_issue_message(issue));
    }
    if crate::macos::has_screen_permission() {
        return Ok(());
    }
    // Triggers the system prompt the first time; afterwards the user has to
    // enable it manually, so point them to the settings window.
    if crate::macos::request_screen_permission() {
        return Ok(());
    }
    windows::show_settings(app);
    let _ = app.emit("screen-permission-missing", ());
    Err("screen recording permission missing".into())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn ensure_permission<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    Ok(())
}

/// Entry point for "capture a region". Safe to call from any thread / the
/// hotkey handler; the heavy lifting runs on a worker thread.
pub fn begin_region<R: Runtime>(app: AppHandle<R>) {
    begin_session(app, false, Mode::Screenshot);
}

/// Start an overlay session. `preselect_full` makes the primary monitor's
/// overlay skip the drag and go straight to annotating the whole screen;
/// `Mode::Record` makes the overlay offer to record the selection instead.
pub(crate) fn begin_session<R: Runtime>(app: AppHandle<R>, preselect_full: bool, mode: Mode) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        if state.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(e) = begin_region_inner(&app, preselect_full, mode) {
            eprintln!("[capture] {e}");
            end_session(&app);
            let _ = app.emit("capture-error", e);
        }
    });
}

fn begin_region_inner<R: Runtime>(app: &AppHandle<R>, preselect_full: bool, mode: Mode) -> Result<(), String> {
    ensure_permission(app)?;
    let state = app.state::<AppState>();
    state.record_mode.store(mode == Mode::Record, Ordering::SeqCst);
    let started = Instant::now();
    *state.started.lock().unwrap() = Some(started);
    let session = state.session.fetch_add(1, Ordering::SeqCst) + 1;

    let shots = capture_monitors()?;
    debug::log(format!(
        "session {session}: captured {} monitor(s) in {:?}",
        shots.len(),
        started.elapsed()
    ));
    for s in &shots {
        debug::log(format!(
            "  monitor {} \"{}\" at ({}, {}) {}x{} logical, scale {}, image {}x{}, primary {}",
            s.geom.id, s.geom.name, s.geom.x, s.geom.y, s.geom.width, s.geom.height,
            s.geom.scale, s.image.width(), s.image.height(), s.geom.primary
        ));
        if debug::options().dump_dir.is_some() {
            if let Ok(png) = encode_png(&s.image) {
                debug::dump(&format!("monitor-{}.png", s.geom.id), &png);
            }
        }
    }

    let geoms: Vec<MonitorGeom> = shots.iter().map(|s| s.geom.clone()).collect();
    let full_target = preselect_full
        .then(|| geoms.iter().find(|g| g.primary).or(geoms.first()).map(|g| g.id))
        .flatten();
    let infos: Vec<MonitorInfo> = shots
        .iter()
        .map(|s| MonitorInfo {
            geom: s.geom.clone(),
            image_width: s.image.width(),
            image_height: s.image.height(),
            session,
            preselect_full: full_target == Some(s.geom.id),
            mode,
        })
        .collect();
    *state.shots.lock().unwrap() = shots;

    windows::ensure_overlays(app, &geoms)?;
    debug::log(format!(
        "session {session}: overlay windows ready in {:?}",
        started.elapsed()
    ));
    windows::start_overlays(app, &infos);

    if let Err(e) = app.global_shortcut().on_shortcut(CANCEL_SHORTCUT, |app, _, event| {
        if event.state == ShortcutState::Pressed {
            // The plugin holds its shortcut table lock while calling us, and
            // `cancel` unregisters this very shortcut: do it off this thread.
            let app = app.clone();
            std::thread::spawn(move || cancel(&app));
        }
    }) {
        debug::log(format!("cannot register global Escape: {e}"));
    }
    spawn_cursor_tracker(app.clone(), session);
    Ok(())
}

/// While a session is active: follow the mouse from the native side, push
/// its position to the overlay under it (for the crosshair) and keep that
/// overlay focused. Stops by itself when the session ends.
fn spawn_cursor_tracker<R: Runtime>(app: AppHandle<R>, session: u64) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let mut last: Option<(u32, i32, i32)> = None;
        let mut focused: Option<u32> = None;
        let mut ticks: u32 = 0;
        loop {
            // Stops once the session ends or a selection is being annotated
            // (the editor owns focus then; following the mouse would steal it).
            if !state.busy.load(Ordering::SeqCst)
                || state.annotating.load(Ordering::SeqCst)
                || state.session.load(Ordering::SeqCst) != session
            {
                break;
            }
            let geoms: Vec<MonitorGeom> = state.shots.lock().unwrap().iter().map(|s| s.geom.clone()).collect();
            if geoms.is_empty() {
                break;
            }
            let hit = cursor::locate(&app, &geoms);
            // Which overlay should own the keyboard: the one under the cursor,
            // falling back to the primary monitor if the cursor is nowhere.
            let target = hit
                .map(|h| h.monitor_id)
                .or_else(|| (ticks > 6).then(|| geoms.iter().find(|g| g.primary).unwrap_or(&geoms[0]).id));
            if let Some(h) = hit {
                let key = (h.monitor_id, h.x.round() as i32, h.y.round() as i32);
                if last != Some(key) {
                    last = Some(key);
                    let _ = app.emit_to(
                        EventTarget::webview_window(windows::overlay_label(h.monitor_id)),
                        "cursor:move",
                        CursorPos { x: h.x, y: h.y },
                    );
                }
            }
            if let Some(id) = target {
                if focused != Some(id) && windows::focus_overlay(&app, id) {
                    focused = Some(id);
                    debug::log(format!("focus -> overlay {id}"));
                }
            }
            if ticks == 30 || ticks == 90 {
                if let Some(id) = focused {
                    debug::log(format!(
                        "overlay {id} is_focused = {}",
                        windows::overlay_is_focused(&app, id)
                    ));
                }
            }
            ticks += 1;
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    });
}

/// An overlay has painted its bitmap: show it. Focus is handled by the
/// cursor tracker once the window is visible.
pub fn overlay_ready<R: Runtime>(app: &AppHandle<R>, monitor_id: u32) {
    let state = app.state::<AppState>();
    let known = state
        .shots
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.geom.id == monitor_id);
    if !known {
        return; // session ended meanwhile
    }
    windows::show_overlay(app, monitor_id);
    let started = *state.started.lock().unwrap();
    if let Some(started) = started {
        debug::log(format!(
            "overlay {monitor_id} visible after {:?}",
            started.elapsed()
        ));
    }
}

/// The overlay on `monitor_id` has a selection and switched to annotating
/// it in place: the other overlays just stay dimmed, the cursor tracker
/// stops and the global Escape fallback goes away (the user clicked in the
/// overlay, so it owns the keyboard and handles Escape itself).
pub fn begin_annotation<R: Runtime>(app: &AppHandle<R>, monitor_id: u32) {
    let state = app.state::<AppState>();
    if !state.busy.load(Ordering::SeqCst) {
        return;
    }
    state.annotating.store(true, Ordering::SeqCst);
    let _ = app.global_shortcut().unregister(CANCEL_SHORTCUT);
    windows::lock_overlays(app, monitor_id);
    windows::focus_overlay(app, monitor_id);
    debug::log(format!("annotating on overlay {monitor_id}"));
}

/// Capture the whole primary monitor (or the first one) and deliver it.
pub fn begin_fullscreen<R: Runtime>(app: AppHandle<R>) {
    if settings::current(&app).after_capture == AfterCapture::Editor {
        // Annotate in place, like a region capture with everything selected.
        begin_session(app, true, Mode::Screenshot);
        return;
    }
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        if state.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let result = (|| -> Result<(), String> {
            ensure_permission(&app)?;
            let shots = capture_monitors()?;
            let shot = shots
                .iter()
                .find(|s| s.geom.primary)
                .or_else(|| shots.first())
                .ok_or("no monitor")?;
            let png = encode_png(&shot.image)?;
            deliver(&app, png)
        })();
        end_session(&app);
        if let Err(e) = result {
            eprintln!("[capture] {e}");
            let _ = app.emit("capture-error", e);
        }
    });
}

/// Called by an overlay window once the user has drawn a rectangle.
pub fn finish_region<R: Runtime>(app: &AppHandle<R>, region: Region) -> Result<(), String> {
    let png = {
        let state = app.state::<AppState>();
        let shots = state.shots.lock().unwrap();
        let shot = shots
            .iter()
            .find(|s| s.geom.id == region.monitor_id)
            .ok_or("capture session expired")?;
        encode_png(&crop(&shot.image, region)?)?
    };
    debug::log(format!("region {:?} -> {} bytes png", region, png.len()));
    debug::dump("crop.png", &png);
    end_session(app);
    deliver(app, png)
}

pub fn cancel<R: Runtime>(app: &AppHandle<R>) {
    end_session(app);
}

/// An overlay session is active.
pub fn is_busy<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<AppState>().busy.load(Ordering::SeqCst)
}

/// The current session's mode (meaningless while idle).
pub fn mode<R: Runtime>(app: &AppHandle<R>) -> Mode {
    if app.state::<AppState>().record_mode.load(Ordering::SeqCst) {
        Mode::Record
    } else {
        Mode::Screenshot
    }
}

fn end_session<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.global_shortcut().unregister(CANCEL_SHORTCUT);
    windows::hide_overlays(app);
    let state = app.state::<AppState>();
    state.shots.lock().unwrap().clear();
    *state.started.lock().unwrap() = None;
    state.annotating.store(false, Ordering::SeqCst);
    state.record_mode.store(false, Ordering::SeqCst);
    state.busy.store(false, Ordering::SeqCst);
}

/// The editor window is gone: drop the PNG kept for it, so the last
/// screenshot does not stay in memory for the rest of the app's life.
pub fn forget_pending<R: Runtime>(app: &AppHandle<R>) {
    *app.state::<AppState>().pending.lock().unwrap() = None;
}

/// "Open in editor" from the in-place editor: the annotated PNG replaces
/// the overlay session and opens in the editor window.
pub fn edit_png<R: Runtime>(app: &AppHandle<R>, png: Vec<u8>) -> Result<(), String> {
    *app.state::<AppState>().pending.lock().unwrap() = Some(png);
    end_session(app);
    windows::open_editor(app)
}

/// "Save as…" from the in-place editor or the editor window: the system
/// dialog picks the destination and the PNG is written there. The dialog is
/// the only way a webview can name a file outside the save folder, and it
/// runs here in Rust, so the webviews never send paths. A running overlay
/// session is ended first so the dialog is not stuck behind the overlays;
/// the dialog blocks, so it runs on a worker thread. Resolves to the saved
/// path, or `None` when the dialog was cancelled; the outcome is also
/// reported through `capture-done` / `capture-error` like the immediate
/// modes.
pub async fn save_png_as<R: Runtime>(app: &AppHandle<R>, png: Vec<u8>) -> Result<Option<PathBuf>, String> {
    if is_busy(app) {
        end_session(app);
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || save_png_dialog(&app, &png))
        .await
        .map_err(|e| format!("save as: {e}"))?
}

fn save_png_dialog<R: Runtime>(app: &AppHandle<R>, png: &[u8]) -> Result<Option<PathBuf>, String> {
    let settings = settings::current(app);
    let dir = settings::save_dir(app);
    let _ = std::fs::create_dir_all(&dir);
    let file_name = stamped_name(&settings.file_prefix, &time_stamp(), "png", 1);

    // macOS: every window of ours may be hidden now, and the plugin's dialog
    // would attach itself as a sheet to the first one (never visible). Use
    // rfd's modal panel directly: it has no parent, temporarily makes the
    // app a regular one and brings the panel to the front.
    #[cfg(target_os = "macos")]
    let picked: Option<PathBuf> = rfd::FileDialog::new()
        .set_title("Save screenshot")
        .set_directory(&dir)
        .set_file_name(&file_name)
        .add_filter("PNG image", &["png"])
        .save_file();
    #[cfg(not(target_os = "macos"))]
    let picked: Option<PathBuf> = app
        .dialog()
        .file()
        .set_title("Save screenshot")
        .set_directory(&dir)
        .set_file_name(&file_name)
        .add_filter("PNG image", &["png"])
        .blocking_save_file()
        .and_then(|p| p.into_path().ok());

    let Some(path) = picked else {
        debug::log("save as: cancelled");
        return Ok(None);
    };
    let result = save_png(app, png, Some(path)).and_then(|path| {
        if settings.copy_on_save {
            copy_png(app, png)?;
        }
        Ok(path)
    });
    match result {
        Ok(path) => {
            debug::log(format!("saved {}", path.display()));
            let _ = app.emit("capture-done", path.to_string_lossy().into_owned());
            Ok(Some(path))
        }
        Err(e) => {
            eprintln!("[capture] {e}");
            let _ = app.emit("capture-error", e.clone());
            Err(e)
        }
    }
}

/// Route a finished PNG according to the "after capture" setting.
fn deliver<R: Runtime>(app: &AppHandle<R>, png: Vec<u8>) -> Result<(), String> {
    let settings = settings::current(app);
    match settings.after_capture {
        AfterCapture::Editor => {
            *app.state::<AppState>().pending.lock().unwrap() = Some(png);
            windows::open_editor(app)
        }
        AfterCapture::Clipboard => {
            copy_png(app, &png)?;
            let _ = app.emit("capture-done", "clipboard");
            Ok(())
        }
        AfterCapture::Save => {
            let path = save_png(app, &png, None)?;
            if settings.copy_on_save {
                copy_png(app, &png)?;
            }
            let _ = app.emit("capture-done", path.to_string_lossy().into_owned());
            Ok(())
        }
        AfterCapture::Upload => {
            // The upload takes a moment: a worker thread does it and the
            // popover reports the link (or the failure).
            crate::share::share_png_async(app, png);
            Ok(())
        }
    }
}

pub fn copy_png<R: Runtime>(app: &AppHandle<R>, png: &[u8]) -> Result<(), String> {
    let image = tauri::image::Image::from_bytes(png).map_err(|e| e.to_string())?;
    app.clipboard()
        .write_image(&image)
        .map_err(|e| format!("clipboard write failed: {e}"))
}

/// Write PNG bytes to `path`, or to a new auto-named file in the save dir.
///
/// `path` only ever comes from the save dialog in `save_png_dialog` (the
/// webviews cannot pass one), so replacing an existing file there is the
/// user's decision; it does have to be a `.png` (a missing extension is
/// added, a different one refused).
pub fn save_png<R: Runtime>(app: &AppHandle<R>, png: &[u8], path: Option<PathBuf>) -> Result<PathBuf, String> {
    let Some(path) = path else {
        let settings = settings::current(app);
        return write_new(&settings::save_dir(app), &settings.file_prefix, "png", png);
    };
    let path = png_path(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, png).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// A user-chosen destination for a PNG: `.png` is added when the extension
/// is missing and any other extension is refused.
fn png_path(mut path: PathBuf) -> Result<PathBuf, String> {
    match path.extension() {
        None => {
            path.set_extension("png");
        }
        Some(ext) if ext.eq_ignore_ascii_case("png") => {}
        Some(_) => return Err(format!("{} is not a .png file", path.display())),
    }
    Ok(path)
}

pub(crate) fn time_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

/// `{prefix}_{stamp}.{ext}`; from the second attempt on `_2`, `_3`… is
/// added before the extension.
fn stamped_name(prefix: &str, stamp: &str, ext: &str, attempt: u32) -> String {
    if attempt <= 1 {
        format!("{prefix}_{stamp}.{ext}")
    } else {
        format!("{prefix}_{stamp}_{attempt}.{ext}")
    }
}

/// How many `_n` names are tried when the time stamp is taken.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// Write `bytes` to a new auto-named file in `dir`. The file is created
/// with `create_new`, which fails instead of replacing anything already
/// there (including a symlink someone planted under that name); the next
/// free `_n` name is used then.
pub(crate) fn write_new(dir: &Path, prefix: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stamp = time_stamp();
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let path = dir.join(stamped_name(prefix, &stamp, ext, attempt));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
        }
    }
    Err(format!("too many files named {prefix}_{stamp} in {}", dir.display()))
}

/// An auto-named path in `dir` for a file another program writes (the
/// recording encoders), reserved here: the empty file is created with
/// `create_new`, so nothing can be slipped in under that name between
/// choosing it and the encoder opening it. Only for encoders that overwrite
/// (ffmpeg `-y`); see `free_path` for the other kind.
pub(crate) fn reserve_path(dir: &Path, prefix: &str, ext: &str) -> Result<PathBuf, String> {
    write_new(dir, prefix, ext, &[])
}

/// A free auto-named path in `dir` for an encoder that refuses to write to
/// an existing path (macOS `screencapture`, which also refuses a symlink
/// there, so a name planted meanwhile only makes the recording fail). This
/// only looks; the name is taken by the encoder, not here.
pub(crate) fn free_path(dir: &Path, prefix: &str, ext: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stamp = time_stamp();
    (1..=MAX_NAME_ATTEMPTS)
        .map(|attempt| dir.join(stamped_name(prefix, &stamp, ext, attempt)))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .ok_or_else(|| format!("too many files named {prefix}_{stamp} in {}", dir.display()))
}

#[cfg(test)]
mod file_tests {
    use super::{png_path, stamped_name, write_new};
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("screenshot-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn names_carry_a_counter_from_the_second_attempt() {
        assert_eq!(stamped_name("Shot", "2026-01-02_03-04-05", "png", 1), "Shot_2026-01-02_03-04-05.png");
        assert_eq!(stamped_name("Shot", "2026-01-02_03-04-05", "png", 2), "Shot_2026-01-02_03-04-05_2.png");
    }

    #[test]
    fn write_new_never_replaces_an_existing_file() {
        let dir = temp_dir("write-new");
        let first = write_new(&dir, "Shot", "png", b"one").unwrap();
        let second = write_new(&dir, "Shot", "png", b"two").unwrap();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("_2.png"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn write_new_does_not_follow_a_planted_symlink() {
        let dir = temp_dir("symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.txt");
        std::fs::write(&target, b"keep").unwrap();
        // Whatever name the next write would pick, make it a symlink first.
        let stamp = super::time_stamp();
        let link = dir.join(stamped_name("Shot", &stamp, "png", 1));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let written = write_new(&dir, "Shot", "png", b"new").unwrap();
        assert_ne!(written, link);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_paths_must_be_png() {
        assert!(png_path(PathBuf::from("/tmp/shot.jpg")).is_err());
        assert_eq!(png_path(PathBuf::from("/tmp/shot")).unwrap(), PathBuf::from("/tmp/shot.png"));
        assert_eq!(png_path(PathBuf::from("/tmp/SHOT.PNG")).unwrap(), PathBuf::from("/tmp/SHOT.PNG"));
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::{
        settings::{Settings, SettingsState},
        test_support::{app_with, geom, image, settings_in, shot, start_session, temp_dir},
    };
    use tauri::Manager;

    fn region(monitor_id: u32, x: u32, y: u32, width: u32, height: u32) -> Region {
        Region { monitor_id, x, y, width, height }
    }

    #[test]
    fn crop_clips_to_the_bitmap_and_rejects_empty_regions() {
        let img = image(100, 50);
        let inner = crop(&img, region(1, 10, 20, 30, 15)).unwrap();
        assert_eq!((inner.width(), inner.height()), (30, 15));
        assert_eq!(inner.get_pixel(0, 0).0, [10, 20, 7, 255]);

        let clipped = crop(&img, region(1, 90, 40, 30, 30)).unwrap();
        assert_eq!((clipped.width(), clipped.height()), (10, 10));

        let outside = crop(&img, region(1, 500, 500, 10, 10)).unwrap();
        assert_eq!((outside.width(), outside.height()), (1, 1));

        assert_eq!(crop(&img, region(1, 0, 0, 0, 10)).unwrap_err(), "empty region");
    }

    #[test]
    fn encode_png_produces_a_decodable_png() {
        let img = image(20, 10);
        let png = encode_png(&img).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let back = xcap::image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (20, 10));
        assert_eq!(back.get_pixel(5, 3).0, [5, 3, 7, 255]);
    }

    #[test]
    fn monitor_geometry_describes_the_attached_displays() {
        let geoms = monitor_geometry().unwrap();
        assert!(!geoms.is_empty());
        assert!(geoms.iter().all(|g| g.width > 0 && g.height > 0 && g.scale > 0.0));
        assert!(geoms.iter().any(|g| g.primary));
    }

    #[test]
    fn finish_region_saves_the_crop_and_ends_the_session() {
        let dir = temp_dir("capture-save");
        let app = app_with(Settings { after_capture: AfterCapture::Save, ..settings_in(&dir) });
        let session = start_session(&app, vec![shot(geom(1, 0, 0, 100, 50, 2.0, true))]);
        assert!(is_busy(&app));
        assert_eq!(mode(&app), Mode::Screenshot);
        assert!(app.state::<AppState>().session.load(Ordering::SeqCst) >= session);

        finish_region(&app, region(1, 10, 10, 40, 20)).unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        let png = std::fs::read(files[0].path()).unwrap();
        let img = xcap::image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (40, 20));
        assert!(!is_busy(&app));
        assert!(app.state::<AppState>().shots.lock().unwrap().is_empty());
    }

    #[test]
    fn finish_region_uploads_in_upload_mode() {
        let dir = temp_dir("capture-upload");
        let fake = crate::share::tests::Fake::new(1000, 5_000_000);
        let base = crate::share::tests::serve_fake(&fake);
        let app = app_with(Settings { after_capture: AfterCapture::Upload, upload_server: base, ..settings_in(&dir) });
        crate::share::reset(app.handle());
        start_session(&app, vec![shot(geom(1, 0, 0, 100, 50, 2.0, true))]);
        finish_region(&app, region(1, 10, 10, 40, 20)).unwrap();
        assert!(!is_busy(&app));
        for _ in 0..300 {
            if crate::share::notice(&app).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let Some(crate::share::Notice::Shared { link, .. }) = crate::share::notice(&app) else {
            panic!("{:?}", crate::share::notice(&app))
        };
        assert!(link.share_url.starts_with("https://socorin.com/s/"));
        assert_eq!(link.kind, crate::share::Kind::Image);
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing is saved locally");
        crate::share::dismiss(&app);
    }

    #[test]
    fn finish_region_opens_the_editor_in_editor_mode() {
        let dir = temp_dir("capture-editor");
        let app = app_with(settings_in(&dir));
        start_session(&app, vec![shot(geom(2, 0, 0, 60, 40, 1.0, true))]);
        finish_region(&app, region(2, 0, 0, 60, 40)).unwrap();
        assert!(app.get_webview_window(windows::EDITOR).is_some());
        let pending = app.state::<AppState>().pending.lock().unwrap().clone().unwrap();
        assert_eq!(&pending[..4], b"\x89PNG");

        // The editor is reused for the next capture.
        start_session(&app, vec![shot(geom(2, 0, 0, 60, 40, 1.0, true))]);
        finish_region(&app, region(2, 0, 0, 10, 10)).unwrap();
        assert_eq!(app.webview_windows().keys().filter(|l| l.as_str() == windows::EDITOR).count(), 1);
    }

    #[test]
    fn finish_region_fails_for_an_unknown_monitor_or_an_empty_crop() {
        let app = app_with(settings_in(&temp_dir("capture-unknown")));
        start_session(&app, vec![shot(geom(1, 0, 0, 10, 10, 1.0, true))]);
        assert_eq!(finish_region(&app, region(9, 0, 0, 5, 5)).unwrap_err(), "capture session expired");
        assert!(is_busy(&app));
        assert_eq!(finish_region(&app, region(1, 0, 0, 0, 5)).unwrap_err(), "empty region");
        cancel(&app);
        assert!(!is_busy(&app));
    }

    #[test]
    fn overlay_ready_and_annotation_only_act_on_a_live_session() {
        let app = app_with(Settings::default());
        overlay_ready(&app, 1); // no session: ignored
        begin_annotation(&app, 1); // not busy: ignored
        assert!(!app.state::<AppState>().annotating.load(Ordering::SeqCst));

        start_session(&app, vec![shot(geom(1, 0, 0, 10, 10, 1.0, true))]);
        app.state::<AppState>().record_mode.store(true, Ordering::SeqCst);
        assert_eq!(mode(&app), Mode::Record);
        overlay_ready(&app, 1);
        overlay_ready(&app, 42);
        end_session(&app);
        assert_eq!(mode(&app), Mode::Screenshot);
        assert!(app.state::<AppState>().started.lock().unwrap().is_none());
    }

    #[test]
    fn edit_png_hands_the_picture_to_the_editor_window() {
        let app = app_with(Settings::default());
        start_session(&app, vec![shot(geom(1, 0, 0, 10, 10, 1.0, true))]);
        edit_png(&app, vec![1, 2, 3]).unwrap();
        assert!(!is_busy(&app));
        assert_eq!(app.state::<AppState>().pending.lock().unwrap().as_deref(), Some(&[1u8, 2, 3][..]));
        assert!(app.get_webview_window(windows::EDITOR).is_some());
    }

    #[test]
    fn save_png_writes_where_asked_or_into_the_save_folder() {
        let dir = temp_dir("capture-save-png");
        let app = app_with(settings_in(&dir));
        let explicit = save_png(&app, b"abc", Some(dir.join("sub").join("pic"))).unwrap();
        assert_eq!(explicit, dir.join("sub").join("pic.png"));
        assert_eq!(std::fs::read(&explicit).unwrap(), b"abc");
        assert!(save_png(&app, b"abc", Some(dir.join("pic.jpg"))).unwrap_err().contains("not a .png"));

        let auto = save_png(&app, b"xyz", None).unwrap();
        assert!(auto.starts_with(&dir));
        assert!(auto.file_name().unwrap().to_string_lossy().starts_with("Socorin_"));
        assert_eq!(std::fs::read(&auto).unwrap(), b"xyz");

        let free = free_path(&dir, "Clip", "mov").unwrap();
        assert!(free.to_string_lossy().ends_with(".mov"));
        assert!(!free.exists());

        // A reserved name exists (empty) right away, and the next one differs.
        let reserved = reserve_path(&dir, "Clip", "mp4").unwrap();
        assert!(reserved.to_string_lossy().ends_with(".mp4"));
        assert_eq!(std::fs::metadata(&reserved).unwrap().len(), 0);
        assert_ne!(reserve_path(&dir, "Clip", "mp4").unwrap(), reserved);
    }

    /// The entry points are re-entrancy guarded: while a session runs they
    /// return without touching the screen (which the tests must never do).
    #[test]
    fn a_running_session_blocks_new_ones() {
        let app = app_with(Settings::default());
        let session = start_session(&app, vec![shot(geom(1, 0, 0, 10, 10, 1.0, true))]);
        begin_region(app.handle().clone());
        begin_fullscreen(app.handle().clone()); // editor mode: annotate in place
        app.state::<SettingsState>().0.lock().unwrap().after_capture = AfterCapture::Clipboard;
        begin_fullscreen(app.handle().clone()); // immediate mode: worker thread
        crate::record::begin_region(app.handle().clone());
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(is_busy(&app));
        assert_eq!(app.state::<AppState>().session.load(Ordering::SeqCst), session);
        assert_eq!(app.state::<AppState>().shots.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_cursor_tracker_stops_once_its_session_is_over() {
        let app = app_with(Settings::default());
        let session = start_session(&app, vec![]);
        // Busy but without bitmaps (the session ended meanwhile).
        spawn_cursor_tracker(app.handle().clone(), session);
        // A stale session number.
        spawn_cursor_tracker(app.handle().clone(), session + 1);
        end_session(&app);
        spawn_cursor_tracker(app.handle().clone(), session);
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(!is_busy(&app));
    }

    #[test]
    fn copy_png_rejects_bytes_that_are_not_an_image() {
        let app = app_with(Settings::default());
        assert!(copy_png(&app, b"not a png").is_err());
    }
}
