//! Screen / region video recording.
//!
//! macOS uses the system `screencapture -v` (H.264 .mov, cursor included,
//! no extra dependency; it inherits the app's Screen Recording permission).
//! Windows and Linux (X11) use `ffmpeg` (see `ffmpeg_binary` for where it
//! is looked for). One recording at a time: starting again stops the
//! running one, stopping finalises the file, reveals it in the file manager
//! and reports `capture-done`.
//!
//! The region is chosen on the capture overlay (`capture::Mode::Record`),
//! which is hidden before the encoder starts so it is never in the video.
//! While a region records, the Record / Cancel bar the overlay showed is
//! replaced, at the very same spot, by the `recorder` window (elapsed time,
//! Stop & copy, Stop, Cancel); a full-screen recording shows no bar at all
//! and is driven from the (red) tray icon's menu. Stopping finalises the
//! file and reveals it, copies it to the clipboard, or discards it.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    capture::{self, MonitorGeom},
    clipboard, debug, settings, tray, windows,
};

/// What to record: a rectangle inside one monitor, in that monitor's own
/// logical (DPI-independent) pixels.
#[derive(Debug, Clone)]
pub struct Area {
    pub monitor: MonitorGeom,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    pub fn full(monitor: &MonitorGeom) -> Self {
        Self {
            monitor: monitor.clone(),
            x: 0.0,
            y: 0.0,
            width: monitor.width as f64,
            height: monitor.height as f64,
        }
    }

    /// Global logical coordinates (what macOS' `screencapture -R` expects).
    #[cfg(target_os = "macos")]
    fn global_logical(&self) -> (f64, f64, f64, f64) {
        (
            self.monitor.x as f64 + self.x,
            self.monitor.y as f64 + self.y,
            self.width,
            self.height,
        )
    }

    /// Global physical pixels (what ffmpeg's grabbers expect). Even sizes,
    /// as yuv420p needs them.
    #[cfg(not(target_os = "macos"))]
    fn global_physical(&self) -> (i64, i64, i64, i64) {
        let s = self.monitor.scale as f64;
        let even = |v: f64| ((v * s).round() as i64).max(2) / 2 * 2;
        (
            ((self.monitor.x as f64 + self.x) * s).round() as i64,
            ((self.monitor.y as f64 + self.y) * s).round() as i64,
            even(self.width),
            even(self.height),
        )
    }
}

/// Where the overlay's Record / Cancel bar is, relative to the monitor's
/// top-left corner in logical pixels. The recording bar goes exactly there.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Bar {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bar {
    /// True when the bar would be inside the recorded area (and so in the
    /// video): then no bar is shown and the tray menu drives the recording.
    pub fn overlaps(&self, area: &Area) -> bool {
        self.x < area.x + area.width
            && area.x < self.x + self.width
            && self.y < area.y + area.height
            && area.y < self.y + self.height
    }
}

/// What to do with the file when a recording stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Keep it and reveal it in the file manager.
    Reveal,
    /// Keep it and put the file on the clipboard, ready to paste into a chat.
    Copy,
    /// Delete it.
    Discard,
}

/// Payload of `recording:stopped`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub copied: bool,
}

pub(crate) struct Active {
    pub(crate) child: Child,
    pub(crate) path: PathBuf,
    pub(crate) started: Instant,
    pub(crate) started_ms: u64,
}

#[derive(Default)]
pub struct RecordState {
    pub(crate) active: Mutex<Option<Active>>,
    /// `start` is between showing the bar and spawning the encoder.
    starting: AtomicBool,
    /// Stop / cancel arrived while starting: undo right after the spawn.
    abort: AtomicBool,
}

/// What the floating bar shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub recording: bool,
    /// Unix time in ms when the recording started (0 when idle).
    pub started_ms: u64,
    pub path: String,
}

pub fn status<R: Runtime>(app: &AppHandle<R>) -> RecordingStatus {
    let state = app.state::<RecordState>();
    let guard = state.active.lock().unwrap();
    match guard.as_ref() {
        Some(a) => RecordingStatus {
            recording: true,
            started_ms: a.started_ms,
            path: a.path.to_string_lossy().into_owned(),
        },
        None => RecordingStatus {
            recording: false,
            started_ms: 0,
            path: String::new(),
        },
    }
}

pub fn is_recording<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<RecordState>().active.lock().unwrap().is_some()
}

/// Tray / hotkey / CLI entry point: choose a region on the overlay and
/// record it. While a recording runs this stops it instead (so one hotkey
/// toggles).
pub fn begin_region<R: Runtime>(app: AppHandle<R>) {
    if is_recording(&app) {
        stop_async(app);
        return;
    }
    capture::begin_session(app, false, capture::Mode::Record);
}

/// Record the whole primary monitor (no overlay). Toggles like `begin_region`.
pub fn begin_fullscreen<R: Runtime>(app: AppHandle<R>) {
    if is_recording(&app) {
        stop_async(app);
        return;
    }
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            capture::ensure_permission(&app)?;
            let geoms = capture::monitor_geometry()?;
            let monitor = geoms.iter().find(|g| g.primary).unwrap_or(&geoms[0]);
            start(&app, Area::full(monitor), None)
        })();
        if let Err(e) = result {
            eprintln!("[record] {e}");
            let _ = app.emit("capture-error", e);
        }
    });
}

/// Start recording `area`. `bar` is where the overlay's Record bar is: the
/// recording bar takes its place (unless it lies inside the area, or there
/// is none), then the capture session ends so the overlays are off screen
/// before the first frame. Blocks for a moment, so call it from a worker
/// thread. A stop or cancel that arrives meanwhile is honoured right after
/// the encoder starts.
pub fn start<R: Runtime>(app: &AppHandle<R>, area: Area, bar: Option<Bar>) -> Result<(), String> {
    let state = app.state::<RecordState>();
    if is_recording(app) || state.starting.swap(true, Ordering::SeqCst) {
        return Err("already recording".into());
    }
    state.abort.store(false, Ordering::SeqCst);
    let result = start_inner(app, area, bar);
    state.starting.store(false, Ordering::SeqCst);
    if result.is_err() {
        windows::hide_recorder(app);
    }
    result
}

fn start_inner<R: Runtime>(app: &AppHandle<R>, area: Area, bar: Option<Bar>) -> Result<(), String> {
    if area.width < 2.0 || area.height < 2.0 {
        return Err("the area to record is too small".into());
    }
    let state = app.state::<RecordState>();
    // The bar goes up first, over the overlay's own bar at the same spot, so
    // the hand-over is seamless when the overlay disappears.
    let bar = bar.filter(|b| !b.overlaps(&area));
    if let Some(b) = bar {
        let _ = app.emit("recording:reset", ());
        windows::show_recorder(app, &area.monitor, &b);
    }
    let had_session = capture::is_busy(app);
    capture::cancel(app);
    if had_session {
        // Let the compositor actually take the dimmed overlays down.
        std::thread::sleep(Duration::from_millis(350));
    }
    if state.abort.load(Ordering::SeqCst) {
        debug::log("recording cancelled before it started");
        finish_abort(app);
        return Ok(());
    }

    let path = output_path(app)?;
    let child = match spawn_backend(app, &area, &path) {
        Ok(child) => child,
        Err(e) => {
            // The name may have been reserved with an empty file.
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };
    let started_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    debug::log(format!(
        "recording {}x{} at ({}, {}) on monitor {} -> {}{}",
        area.width, area.height, area.x, area.y, area.monitor.id, path.display(),
        if bar.is_some() { " (bar)" } else { " (tray only)" }
    ));
    *state.active.lock().unwrap() = Some(Active {
        child,
        path,
        started: Instant::now(),
        started_ms,
    });
    if state.abort.load(Ordering::SeqCst) {
        debug::log("recording cancelled while it started");
        let _ = stop_with(app, Outcome::Discard);
        return Ok(());
    }
    tray::set_recording(app, true);
    let _ = app.emit("recording:started", status(app));
    Ok(())
}

/// A stop / cancel came in before the encoder existed: just take the bar
/// down again.
fn finish_abort<R: Runtime>(app: &AppHandle<R>) {
    windows::hide_recorder(app);
    let _ = app.emit("recording:stopped", Stopped { copied: false });
}

pub fn stop_async<R: Runtime>(app: AppHandle<R>) {
    stop_async_with(app, Outcome::Reveal);
}

pub fn stop_async_with<R: Runtime>(app: AppHandle<R>, outcome: Outcome) {
    std::thread::spawn(move || {
        if let Err(e) = stop_with(&app, outcome) {
            eprintln!("[record] {e}");
            let _ = app.emit("capture-error", e);
        }
    });
}

/// Stop the running recording, revealing the file.
pub fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<Option<PathBuf>, String> {
    stop_with(app, Outcome::Reveal)
}

/// Stop the running recording and wait for the encoder to finalise the
/// file (blocks up to ~20 s). Returns the kept file, or `None` when it was
/// discarded or the recording had not started yet (then it is cancelled as
/// soon as it does).
pub fn stop_with<R: Runtime>(app: &AppHandle<R>, outcome: Outcome) -> Result<Option<PathBuf>, String> {
    let state = app.state::<RecordState>();
    let active = state.active.lock().unwrap().take();
    let Some(active) = active else {
        if state.starting.load(Ordering::SeqCst) {
            state.abort.store(true, Ordering::SeqCst);
            return Ok(None);
        }
        return Err("not recording".into());
    };
    tray::set_recording(app, false);
    if outcome != Outcome::Copy {
        windows::hide_recorder(app);
    }

    let Active {
        mut child,
        path,
        started,
        ..
    } = active;
    signal_stop(&mut child);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                eprintln!("[record] encoder did not exit, killing it");
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    if outcome == Outcome::Discard {
        let _ = std::fs::remove_file(&path);
        debug::log(format!("discarded {} after {:?}", path.display(), started.elapsed()));
        let _ = app.emit("recording:stopped", Stopped { copied: false });
        return Ok(None);
    }
    // Only a regular file at that name counts (a symlink planted there is
    // not revealed or copied; `symlink_metadata` does not follow it).
    let size = std::fs::symlink_metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&path);
        windows::hide_recorder(app);
        let _ = app.emit("recording:stopped", Stopped { copied: false });
        return Err("recording failed: no video was written (is screen recording allowed?)".into());
    }
    debug::log(format!(
        "recorded {} ({} bytes, {:?})",
        path.display(),
        size,
        started.elapsed()
    ));
    let copied = if outcome == Outcome::Copy {
        match clipboard::copy_file(app, &path) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[record] cannot copy the recording: {e}");
                let _ = app.emit("capture-error", format!("Saved, but could not copy it to the clipboard: {e}"));
                false
            }
        }
    } else {
        false
    };
    if copied {
        // The bar says "Copied" for a moment before it goes.
        windows::hide_recorder_later(app, Duration::from_millis(1500));
    } else {
        windows::hide_recorder(app);
        let _ = tauri_plugin_opener::reveal_item_in_dir(&path);
    }
    let _ = app.emit("recording:stopped", Stopped { copied });
    let _ = app.emit("capture-done", path.to_string_lossy().into_owned());
    Ok(Some(path))
}

/// Stop synchronously if a recording runs (used right before quitting, so
/// no orphaned encoder keeps writing).
pub fn stop_if_recording<R: Runtime>(app: &AppHandle<R>) {
    if is_recording(app) {
        let _ = stop(app);
    }
}

/// Where the encoder writes. ffmpeg (`-y`) overwrites, so the name is
/// reserved with an empty file first and nothing else can take it in
/// between. macOS `screencapture` refuses an existing path — a symlink
/// included, so there a planted name only makes the recording fail — and
/// gets a name that is merely free.
fn output_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let settings = settings::current(app);
    let dir = settings::save_dir(app);
    if cfg!(target_os = "macos") {
        capture::free_path(&dir, &settings.file_prefix, "mov")
    } else {
        capture::reserve_path(&dir, &settings.file_prefix, "mp4")
    }
}

/// The ffmpeg the user entered under Settings → Recording. The recorder
/// runs whatever this names, so it has to be an absolute path to an
/// existing file called `ffmpeg` (or `ffmpeg.exe`): the setting cannot
/// point the recorder at some other program.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn configured_ffmpeg(configured: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(configured);
    let stem_is_ffmpeg = path
        .file_stem()
        .map(|s| s.eq_ignore_ascii_case("ffmpeg"))
        .unwrap_or(false);
    let ext_is_exe_or_none = path.extension().map_or(true, |e| e.eq_ignore_ascii_case("exe"));
    if !(stem_is_ffmpeg && ext_is_exe_or_none) {
        return Err(format!(
            "{configured} is not an ffmpeg binary: Settings → Recording expects the path of ffmpeg itself (…/ffmpeg or …\\ffmpeg.exe)."
        ));
    }
    if path.is_absolute() && path.is_file() {
        Ok(path)
    } else {
        Err(format!("ffmpeg not found at {configured} (Settings → Recording); leave the field empty to search for it."))
    }
}

fn quiet() -> Stdio {
    if debug::options().enabled {
        Stdio::inherit()
    } else {
        Stdio::null()
    }
}

#[cfg(target_os = "macos")]
fn spawn_backend<R: Runtime>(_app: &AppHandle<R>, area: &Area, path: &Path) -> Result<Child, String> {
    let (x, y, w, h) = area.global_logical();
    let rect = format!("{},{},{},{}", x.round(), y.round(), w.round(), h.round());
    // -v video, -x no shutter sound, -R rectangle in global points.
    Command::new("/usr/sbin/screencapture")
        .args(["-v", "-x", "-R", &rect])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(quiet())
        .stderr(quiet())
        .spawn()
        .map_err(|e| format!("cannot start screencapture: {e}"))
}

/// The ffmpeg binary to run. The setting wins when it is filled in; otherwise
/// the usual install locations are tried, then the directories on PATH. The
/// result is always an absolute path: a bare name would let a relative PATH
/// entry (or, on Windows, the working directory) supply an impostor.
#[cfg(not(target_os = "macos"))]
fn ffmpeg_binary<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let configured = settings::current(app).ffmpeg_path;
    if !configured.is_empty() {
        return configured_ffmpeg(&configured);
    }
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "windows")]
    {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                candidates.push(PathBuf::from(dir).join("ffmpeg").join("bin").join(exe));
            }
        }
        if let Some(dir) = std::env::var_os("ProgramData") {
            candidates.push(PathBuf::from(dir).join("chocolatey").join("bin").join(exe));
        }
        if let Some(dir) = std::env::var_os("SystemDrive") {
            candidates.push(PathBuf::from(format!("{}\\ffmpeg\\bin", dir.to_string_lossy())).join(exe));
        }
    }
    #[cfg(target_os = "linux")]
    for dir in ["/usr/bin", "/usr/local/bin", "/snap/bin", "/var/lib/flatpak/exports/bin"] {
        candidates.push(PathBuf::from(dir).join(exe));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(exe)),
        );
    }
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        "Screen recording needs ffmpeg: install it, or enter its location under Settings → Recording."
            .to_string()
    })
}

#[cfg(not(target_os = "macos"))]
fn spawn_backend<R: Runtime>(app: &AppHandle<R>, area: &Area, path: &Path) -> Result<Child, String> {
    #[cfg(target_os = "linux")]
    if windows::is_wayland() {
        return Err("Screen recording needs an X11 session; Wayland is not supported yet.".into());
    }
    let binary = ffmpeg_binary(app)?;
    debug::log(format!("ffmpeg: {}", binary.display()));
    let (x, y, w, h) = area.global_physical();
    let mut cmd = Command::new(&binary);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    #[cfg(target_os = "windows")]
    cmd.args([
        "-f", "gdigrab", "-framerate", "30",
        "-offset_x", &x.to_string(), "-offset_y", &y.to_string(),
        "-video_size", &format!("{w}x{h}"), "-i", "desktop",
    ]);
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        cmd.args([
            "-f", "x11grab", "-framerate", "30",
            "-video_size", &format!("{w}x{h}"),
            "-i", &format!("{display}+{x},{y}"),
        ]);
    }
    cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(quiet())
        .stderr(quiet());
    cmd.spawn()
        .map_err(|e| format!("cannot start {}: {e}", binary.display()))
}

/// Ask the encoder to finish (it then writes the file's index / trailer).
#[cfg(unix)]
fn signal_stop(child: &mut Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
}

#[cfg(windows)]
fn signal_stop(child: &mut Child) {
    use std::io::Write;
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{app_with, geom, settings_in, sleeping_child, temp_dir};
    use tauri::Manager;

    fn recording(app: &AppHandle<tauri::test::MockRuntime>, path: PathBuf) {
        *app.state::<RecordState>().active.lock().unwrap() = Some(Active {
            child: sleeping_child(),
            path,
            started: Instant::now(),
            started_ms: 1_700_000_000_000,
        });
    }

    #[test]
    fn areas_cover_the_monitor_and_convert_to_global_points() {
        let monitor = geom(1, 100, 50, 640, 480, 2.0, true);
        let full = Area::full(&monitor);
        assert_eq!((full.x, full.y, full.width, full.height), (0.0, 0.0, 640.0, 480.0));
        #[cfg(target_os = "macos")]
        {
            let area = Area { monitor: monitor.clone(), x: 10.0, y: 20.0, width: 30.0, height: 40.0 };
            assert_eq!(area.global_logical(), (110.0, 70.0, 30.0, 40.0));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let area = Area { monitor: monitor.clone(), x: 10.0, y: 20.0, width: 31.0, height: 40.0 };
            assert_eq!(area.global_physical(), (220, 140, 62, 80));
        }
        let _ = monitor;
    }

    #[test]
    fn a_bar_inside_the_area_would_be_in_the_video() {
        let area = Area { monitor: geom(1, 0, 0, 1000, 800, 1.0, true), x: 100.0, y: 100.0, width: 300.0, height: 200.0 };
        // Just below the area, right-aligned: outside.
        assert!(!Bar { x: 0.0, y: 308.0, width: 400.0, height: 42.0 }.overlaps(&area));
        // Touching the bottom edge exactly: still outside.
        assert!(!Bar { x: 100.0, y: 300.0, width: 300.0, height: 42.0 }.overlaps(&area));
        // Left of it: outside.
        assert!(!Bar { x: 0.0, y: 150.0, width: 100.0, height: 42.0 }.overlaps(&area));
        // Overlapping a corner / fully inside: in the video.
        assert!(Bar { x: 350.0, y: 250.0, width: 400.0, height: 42.0 }.overlaps(&area));
        assert!(Bar { x: 150.0, y: 150.0, width: 50.0, height: 20.0 }.overlaps(&area));
    }

    #[test]
    fn status_reflects_the_running_recording() {
        let dir = temp_dir("record-status");
        let app = app_with(settings_in(&dir));
        let idle = status(&app);
        assert!(!idle.recording && idle.started_ms == 0 && idle.path.is_empty());
        assert!(!is_recording(&app));
        assert_eq!(stop(&app).unwrap_err(), "not recording");
        stop_if_recording(&app);

        recording(&app, dir.join("clip.mov"));
        let live = status(&app);
        assert!(live.recording);
        assert_eq!(live.started_ms, 1_700_000_000_000);
        assert!(live.path.ends_with("clip.mov"));
        assert!(is_recording(&app));
        assert_eq!(start(&app, Area::full(&geom(1, 0, 0, 100, 100, 1.0, true)), None).unwrap_err(), "already recording");
    }

    #[test]
    fn a_stop_while_starting_is_honoured_once_the_encoder_exists() {
        let dir = temp_dir("record-abort");
        let app = app_with(settings_in(&dir));
        let state = app.state::<RecordState>();
        state.starting.store(true, Ordering::SeqCst);
        assert_eq!(stop_with(&app, Outcome::Reveal).unwrap(), None);
        assert!(state.abort.load(Ordering::SeqCst));
        // `start` sees the flag and cancels instead of recording.
        state.starting.store(false, Ordering::SeqCst);
        assert!(!is_recording(&app));
        finish_abort(&app);
    }

    #[test]
    fn a_stop_during_the_start_up_delay_cancels_before_the_encoder_exists() {
        let dir = temp_dir("record-early-stop");
        let app = app_with(settings_in(&dir));
        let monitor = geom(1, 0, 0, 200, 200, 1.0, true);
        // The overlay chose the area, so `start` first takes the overlays
        // down and waits for the compositor; a stop that lands in that
        // window must win, and no encoder may be started afterwards.
        crate::test_support::start_session(&app, vec![crate::test_support::shot(monitor.clone())]);
        let handle = app.handle().clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            stop_with(&handle, Outcome::Reveal)
        });
        let area = Area { monitor, x: 10.0, y: 10.0, width: 100.0, height: 80.0 };
        let bar = Bar { x: 10.0, y: 98.0, width: 400.0, height: 42.0 };
        assert_eq!(start(&app, area, Some(bar)), Ok(()));
        assert_eq!(stopper.join().unwrap(), Ok(None));
        assert!(!is_recording(&app));
        assert!(!capture::is_busy(&app));
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing may be written");
    }

    #[test]
    fn a_save_folder_that_cannot_be_created_fails_before_the_encoder() {
        let dir = temp_dir("record-bad-dir");
        let blocker = dir.join("not-a-folder");
        std::fs::write(&blocker, b"x").unwrap();
        let app = app_with(settings_in(&blocker));
        let monitor = geom(1, 0, 0, 200, 200, 1.0, true);
        let area = Area { monitor, x: 0.0, y: 0.0, width: 100.0, height: 80.0 };
        // A bar inside the area is dropped (it would be in the video).
        let bar = Bar { x: 10.0, y: 10.0, width: 50.0, height: 20.0 };
        let err = start(&app, area, Some(bar)).unwrap_err();
        assert!(err.contains("cannot create"), "{err}");
        assert!(!is_recording(&app));
        assert!(!app.state::<RecordState>().starting.load(Ordering::SeqCst));
    }

    #[test]
    fn cancelling_discards_the_file() {
        let dir = temp_dir("record-discard");
        let app = app_with(settings_in(&dir));
        let path = dir.join("junk.mov");
        std::fs::write(&path, b"data").unwrap();
        recording(&app, path.clone());
        assert_eq!(stop_with(&app, Outcome::Discard).unwrap(), None);
        assert!(!path.exists());
        assert!(!is_recording(&app));
    }

    #[test]
    fn stopping_signals_the_encoder_and_reports_an_empty_file() {
        let dir = temp_dir("record-stop");
        let app = app_with(settings_in(&dir));
        let path = dir.join("empty.mov");
        std::fs::write(&path, b"").unwrap();
        recording(&app, path.clone());
        let err = stop(&app).unwrap_err();
        assert!(err.contains("no video was written"), "{err}");
        assert!(!is_recording(&app));
        std::fs::write(&path, b"").unwrap();
        recording(&app, path.clone());
        let err = stop_with(&app, Outcome::Copy).unwrap_err();
        assert!(err.contains("no video was written"), "{err}");
        assert!(!path.exists());
        assert!(!is_recording(&app));
    }

    #[test]
    fn the_toggles_stop_a_running_recording() {
        let dir = temp_dir("record-toggle");
        let app = app_with(settings_in(&dir));
        recording(&app, dir.join("a.mov"));
        begin_region(app.handle().clone());
        wait_until_idle(&app);
        recording(&app, dir.join("b.mov"));
        begin_fullscreen(app.handle().clone());
        wait_until_idle(&app);
        recording(&app, dir.join("c.mov"));
        stop_if_recording(&app);
        assert!(!is_recording(&app));
        stop_async(app.handle().clone());
        std::thread::sleep(Duration::from_millis(50));
    }

    fn wait_until_idle(app: &AppHandle<tauri::test::MockRuntime>) {
        for _ in 0..200 {
            if !is_recording(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("recording did not stop");
    }

    #[test]
    fn rejects_areas_that_are_too_small() {
        let app = app_with(settings_in(&temp_dir("record-small")));
        let monitor = geom(1, 0, 0, 100, 100, 1.0, true);
        let err = start(&app, Area { monitor, x: 0.0, y: 0.0, width: 1.0, height: 50.0 }, None).unwrap_err();
        assert!(err.contains("too small"));
        assert!(!app.state::<RecordState>().starting.load(Ordering::SeqCst));
        stop_async_with(app.handle().clone(), Outcome::Discard);
        std::thread::sleep(Duration::from_millis(30));
    }

    #[test]
    fn output_files_go_to_the_save_folder() {
        let dir = temp_dir("record-output");
        let app = app_with(settings_in(&dir));
        let path = output_path(&app).unwrap();
        assert!(path.starts_with(&dir));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Socorin_"));
        assert!(name.ends_with(if cfg!(target_os = "macos") { ".mov" } else { ".mp4" }));
        // ffmpeg overwrites, so its name is reserved up front; screencapture
        // would refuse an existing file, so on macOS the name is only free.
        assert_eq!(path.exists(), !cfg!(target_os = "macos"));
        let _ = quiet();
    }

    /// A symlink planted under the recording's name is neither revealed nor
    /// copied: only a regular file counts as a recording.
    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_output_path_is_not_a_recording() {
        let dir = temp_dir("record-symlink");
        let app = app_with(settings_in(&dir));
        let target = dir.join("elsewhere.mov");
        std::fs::write(&target, b"not ours").unwrap();
        let link = dir.join("planted.mov");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        recording(&app, link.clone());
        let err = stop_with(&app, Outcome::Copy).unwrap_err();
        assert!(err.contains("no video was written"), "{err}");
        assert!(std::fs::symlink_metadata(&link).is_err(), "the link is removed");
        assert_eq!(std::fs::read(&target).unwrap(), b"not ours");
    }

    #[test]
    fn the_configured_ffmpeg_must_be_an_existing_ffmpeg() {
        let dir = temp_dir("record-ffmpeg");
        let real = dir.join("ffmpeg");
        std::fs::write(&real, b"").unwrap();
        assert_eq!(configured_ffmpeg(real.to_str().unwrap()).unwrap(), real);
        let exe = dir.join("FFmpeg.EXE");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(configured_ffmpeg(exe.to_str().unwrap()).unwrap(), exe);

        let other = dir.join("sh");
        std::fs::write(&other, b"").unwrap();
        for wrong in [other.to_str().unwrap(), dir.join("ffmpeg.sh").to_str().unwrap(), dir.to_str().unwrap()] {
            let err = configured_ffmpeg(wrong).unwrap_err();
            assert!(err.contains("not an ffmpeg"), "{wrong}: {err}");
        }
        let err = configured_ffmpeg(dir.join("missing").join("ffmpeg").to_str().unwrap()).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        let err = configured_ffmpeg("ffmpeg").unwrap_err();
        assert!(err.contains("not found"), "relative names are not searched: {err}");
        let err = configured_ffmpeg(dir.join("ffmpeg-dir").to_str().unwrap()).unwrap_err();
        assert!(err.contains("not an ffmpeg"), "{err}");
    }
}
