//! Screen / region video recording.
//!
//! macOS uses the system `screencapture -v` (H.264 .mov, cursor included,
//! no extra dependency; it inherits the app's Screen Recording permission).
//! Windows records in-process with Windows.Graphics.Capture and Media
//! Foundation (`wgc`), with `ffmpeg` as the fallback (and as the recorder
//! when Settings → Recording names one). Linux (X11) uses `ffmpeg` (see
//! `ffmpeg_binary` for where it is looked for). One recording at a time:
//! starting again stops the running one, stopping finalises the file,
//! reveals it in the file manager and reports `capture-done`.
//!
//! The region is chosen on the capture overlay (`capture::Mode::Record`),
//! which is hidden before the encoder starts so it is never in the video.
//! While a region records, the Record / Cancel bar the overlay showed is
//! replaced, at the very same spot, by the `recorder` window (elapsed time,
//! Stop & copy, Stop & upload, Stop, Cancel); a full-screen recording shows
//! no bar at all and is driven from the (red) tray icon's menu. Stopping
//! finalises the file and reveals it, copies it to the clipboard, uploads
//! it to the share server (`share`), or discards it.

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
    clipboard, debug, settings, share, tray, windows,
};
#[cfg(windows)]
use crate::wgc;

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
    /// Keep it, upload it to the share server and put the link on the
    /// clipboard (see `upload_recording`).
    Upload,
    /// Delete it.
    Discard,
}

/// Payload of `recording:stopped`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub copied: bool,
    /// The share link, after Stop & upload.
    pub link: Option<String>,
}

pub(crate) struct Active {
    pub(crate) backend: Backend,
    pub(crate) path: PathBuf,
    pub(crate) started: Instant,
    pub(crate) started_ms: u64,
}

/// The running encoder: a child process (macOS `screencapture`, ffmpeg) or
/// the in-process Windows recorder.
pub(crate) enum Backend {
    Process(Child),
    #[cfg(windows)]
    Native(wgc::Recorder),
}

impl Backend {
    /// Asks the encoder to finish and waits, `timeout` at most, for it to
    /// finalise the file. A process that will not stop is killed.
    fn finish(self, timeout: Duration) -> Result<(), String> {
        match self {
            Backend::Process(mut child) => {
                signal_stop(&mut child);
                let deadline = Instant::now() + timeout;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => return Ok(()),
                        Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err("the encoder did not exit and was killed".into());
                        }
                    }
                }
            }
            #[cfg(windows)]
            Backend::Native(recorder) => recorder.stop(timeout),
        }
    }

    /// Drops the recording at once, without finalising the file (the tests
    /// reset the shared app between cases).
    #[cfg(test)]
    pub(crate) fn abandon(self) {
        match self {
            Backend::Process(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(windows)]
            Backend::Native(recorder) => recorder.abandon(),
        }
    }
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
    let backend = match spawn_backend(app, &area, &path) {
        Ok(backend) => backend,
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
        backend,
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
    let _ = app.emit("recording:stopped", Stopped { copied: false, link: None });
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
    if !matches!(outcome, Outcome::Copy | Outcome::Upload) {
        windows::hide_recorder(app);
    }

    let Active {
        backend,
        path,
        started,
        ..
    } = active;
    let finished = backend.finish(Duration::from_secs(20));
    if let Err(e) = &finished {
        eprintln!("[record] {e}");
    }
    if outcome == Outcome::Discard {
        let _ = std::fs::remove_file(&path);
        debug::log(format!("discarded {} after {:?}", path.display(), started.elapsed()));
        let _ = app.emit("recording:stopped", Stopped { copied: false, link: None });
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
        let _ = app.emit("recording:stopped", Stopped { copied: false, link: None });
        return Err(match finished {
            Err(e) => format!("recording failed: {e}"),
            Ok(()) => "recording failed: no video was written (is screen recording allowed?)".into(),
        });
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
    // Stop & upload: the bar says "Uploading…" meanwhile; the popover
    // reports the link or the failure. A file that cannot go (too large,
    // not convertible) is kept and revealed like a plain Stop.
    let (path, link) = if outcome == Outcome::Upload {
        let _ = app.emit("recording:uploading", ());
        let (path, outcome) = upload_recording(app, path);
        let link = share::announce(app, outcome).ok().map(|r| r.share_url);
        (path, link)
    } else {
        (path, None)
    };
    if copied || link.is_some() {
        // The bar says "Copied" / "Link copied" for a moment before it goes.
        windows::hide_recorder_later(app, Duration::from_millis(if copied { 1500 } else { 2500 }));
    } else {
        windows::hide_recorder(app);
        let _ = tauri_plugin_opener::reveal_item_in_dir(&path);
    }
    let _ = app.emit("recording:stopped", Stopped { copied, link });
    let _ = app.emit("capture-done", path.to_string_lossy().into_owned());
    Ok(Some(path))
}

/// Upload a finished recording: converted to MP4 first when it is a
/// QuickTime file (what macOS writes), checked against the server's limits
/// before it is read into memory, then sent like a screenshot. Returns the
/// file that is on disk afterwards (the `.mp4` after a conversion) and the
/// link, or why there is none.
fn upload_recording<R: Runtime>(app: &AppHandle<R>, path: PathBuf) -> (PathBuf, Result<share::SharedLink, share::ShareError>) {
    let path = match uploadable_video(app, path) {
        Ok(path) => path,
        Err((path, e)) => return (path, Err(e)),
    };
    let Some(mime) = share::video_mime(&path) else {
        return (path.clone(), Err(share::ShareError::new("unsupported_type", MOV_HELP)));
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let result = tauri::async_runtime::block_on(async {
        let limits = share::limits(app).await?;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        share::check(&limits, mime, size, &settings::current(app).upload_server)?;
        let bytes = std::fs::read(&path).map_err(|e| share::ShareError::new("io", format!("cannot read {}: {e}", path.display())))?;
        share::upload(app, bytes, share::Kind::Video, mime, name).await
    });
    (path, result)
}

/// Said when a `.mov` cannot be converted.
const MOV_HELP: &str = "macOS records QuickTime .mov files and the share server accepts MP4 or WebM only. Install ffmpeg (brew install ffmpeg) or name it under Settings → Recording, and Socorin converts recordings to MP4 before uploading.";

/// The file the share server accepts: an MP4 or WebM as it is; a `.mov`
/// remuxed into an `.mp4` (no re-encoding) when an ffmpeg is at hand, else
/// refused with an explanation. Anything else is refused outright. On
/// failure the path handed back is the file still on disk.
pub(crate) fn uploadable_video<R: Runtime>(app: &AppHandle<R>, path: PathBuf) -> Result<PathBuf, (PathBuf, share::ShareError)> {
    if share::video_mime(&path).is_some() {
        return Ok(path);
    }
    let is_mov = path.extension().map_or(false, |e| e.eq_ignore_ascii_case("mov"));
    if !is_mov {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err((path, share::ShareError::new("unsupported_type", format!("{name} is not an MP4 or WebM file, which is what the share server accepts."))));
    }
    let ffmpeg = match ffmpeg_binary(app) {
        Ok(ffmpeg) => ffmpeg,
        Err(e) => {
            eprintln!("[record] {e}");
            return Err((path, share::ShareError::new("unsupported_type", MOV_HELP)));
        }
    };
    match remux_to_mp4(&ffmpeg, &path) {
        Ok(mp4) => Ok(mp4),
        Err(e) => Err((path, share::ShareError::new("convert", e))),
    }
}

/// `ffmpeg -c copy`: the H.264 stream goes into an MP4 container as it is
/// (no re-encoding, well under a second for a short clip). The `.mov` is
/// replaced by the `.mp4`; a name that is taken gets `_2`, `_3`, ….
pub(crate) fn remux_to_mp4(ffmpeg: &Path, source: &Path) -> Result<PathBuf, String> {
    let target = mp4_sibling(source);
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(source)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(quiet())
        .stderr(quiet())
        .status()
        .map_err(|e| format!("cannot start {}: {e}", ffmpeg.display()))?;
    let written = status.success() && std::fs::metadata(&target).map(|m| m.is_file() && m.len() > 0).unwrap_or(false);
    if !written {
        let _ = std::fs::remove_file(&target);
        return Err(format!("{} could not convert {} to MP4.", ffmpeg.display(), source.display()));
    }
    debug::log(format!("converted {} -> {}", source.display(), target.display()));
    let _ = std::fs::remove_file(source);
    Ok(target)
}

/// `name.mp4` next to `name.mov`, or `name_2.mp4`… when that exists.
fn mp4_sibling(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into());
    let dir = source.parent().unwrap_or(Path::new("."));
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem}.mp4") } else { format!("{stem}_{n}.mp4") };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Stop synchronously if a recording runs (used right before quitting, so
/// no orphaned encoder keeps writing).
pub fn stop_if_recording<R: Runtime>(app: &AppHandle<R>) {
    if is_recording(app) {
        let _ = stop(app);
    }
}

/// Where the encoder writes. ffmpeg (`-y`) and the Windows encoder
/// overwrite, so the name is reserved with an empty file first and nothing
/// else can take it in between. macOS `screencapture` refuses an existing
/// path — a symlink included, so there a planted name only makes the
/// recording fail — and gets a name that is merely free.
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
/// (and, on macOS, the MP4 conversion before an upload) runs whatever this
/// names, so it has to be an absolute path to an existing file called
/// `ffmpeg` (or `ffmpeg.exe`): the setting cannot point the app at some
/// other program.
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
fn spawn_backend<R: Runtime>(_app: &AppHandle<R>, area: &Area, path: &Path) -> Result<Backend, String> {
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
        .map(Backend::Process)
        .map_err(|e| format!("cannot start screencapture: {e}"))
}

/// The ffmpeg binary to run. The setting wins when it is filled in; otherwise
/// the usual install locations are tried, then the directories on PATH. The
/// result is always an absolute path: a bare name would let a relative PATH
/// entry (or, on Windows, the working directory) supply an impostor. On
/// macOS ffmpeg only converts recordings for uploading (Homebrew's
/// locations are looked at); the recorder itself is `screencapture`.
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
    #[cfg(target_os = "macos")]
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
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

#[cfg(target_os = "linux")]
fn spawn_backend<R: Runtime>(app: &AppHandle<R>, area: &Area, path: &Path) -> Result<Backend, String> {
    if windows::is_wayland() {
        return Err("Screen recording needs an X11 session; Wayland is not supported yet.".into());
    }
    spawn_ffmpeg(app, area, path).map(Backend::Process)
}

/// The built-in recorder, unless Settings → Recording names an ffmpeg (then
/// that one records, as before). When the built-in recorder cannot start
/// (Windows before 10 1903, no Direct3D device or Media Foundation), an
/// installed ffmpeg takes over.
#[cfg(target_os = "windows")]
fn spawn_backend<R: Runtime>(app: &AppHandle<R>, area: &Area, path: &Path) -> Result<Backend, String> {
    if settings::current(app).ffmpeg_path.is_empty() {
        match wgc::Recorder::start(area, path) {
            Ok(recorder) => return Ok(Backend::Native(recorder)),
            Err(e) => {
                eprintln!("[record] built-in recorder: {e}");
                if ffmpeg_binary(app).is_err() {
                    return Err(format!(
                        "{e}. Installing ffmpeg (Settings → Recording) gives an alternative recorder."
                    ));
                }
            }
        }
    }
    spawn_ffmpeg(app, area, path).map(Backend::Process)
}

/// ffmpeg grabbing the area (`gdigrab` on Windows, `x11grab` on Linux) into
/// an H.264 MP4; `q` on stdin (Windows) or SIGINT ends it cleanly.
#[cfg(not(target_os = "macos"))]
fn spawn_ffmpeg<R: Runtime>(app: &AppHandle<R>, area: &Area, path: &Path) -> Result<Child, String> {
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
            backend: Backend::Process(sleeping_child()),
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

    /// A stand-in for ffmpeg: a shell script that copies its input (`-i X`)
    /// to its last argument, or fails.
    #[cfg(unix)]
    fn fake_ffmpeg(dir: &Path, succeed: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("ffmpeg");
        let body = if succeed {
            "#!/bin/sh\nsrc=\"$6\"\neval \"dst=\\${$#}\"\ncp \"$src\" \"$dst\"\n"
        } else {
            "#!/bin/sh\nexit 1\n"
        };
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn mp4_names_sit_next_to_the_mov_and_never_replace_a_file() {
        let dir = temp_dir("record-mp4-name");
        let mov = dir.join("Socorin_2026.mov");
        assert_eq!(mp4_sibling(&mov), dir.join("Socorin_2026.mp4"));
        std::fs::write(dir.join("Socorin_2026.mp4"), b"taken").unwrap();
        assert_eq!(mp4_sibling(&mov), dir.join("Socorin_2026_2.mp4"));
        std::fs::write(dir.join("Socorin_2026_2.mp4"), b"taken").unwrap();
        assert_eq!(mp4_sibling(&mov), dir.join("Socorin_2026_3.mp4"));
        assert!(mp4_sibling(Path::new("bare")).ends_with("bare.mp4"));
    }

    #[cfg(unix)]
    #[test]
    fn a_mov_is_remuxed_with_ffmpeg_and_replaced_by_the_mp4() {
        let dir = temp_dir("record-remux");
        let mov = dir.join("clip.mov");
        std::fs::write(&mov, b"quicktime bytes").unwrap();
        let ffmpeg = fake_ffmpeg(&dir, true);
        let mp4 = remux_to_mp4(&ffmpeg, &mov).unwrap();
        assert_eq!(mp4, dir.join("clip.mp4"));
        assert_eq!(std::fs::read(&mp4).unwrap(), b"quicktime bytes");
        assert!(!mov.exists(), "the .mov is replaced");

        // ffmpeg fails: the .mov stays, nothing else is left behind.
        std::fs::write(&mov, b"again").unwrap();
        let broken = fake_ffmpeg(&temp_dir("record-remux-broken"), false);
        let err = remux_to_mp4(&broken, &mov).unwrap_err();
        assert!(err.contains("could not convert"), "{err}");
        assert!(mov.exists());
        assert!(!dir.join("clip_2.mp4").exists());
        let err = remux_to_mp4(Path::new("/nonexistent/ffmpeg"), &mov).unwrap_err();
        assert!(err.contains("cannot start"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn only_mp4_and_webm_go_up_a_mov_is_converted_first() {
        let dir = temp_dir("record-uploadable");
        let app = app_with(settings_in(&dir));
        let mp4 = dir.join("a.mp4");
        assert_eq!(uploadable_video(&app, mp4.clone()).unwrap(), mp4);
        let webm = dir.join("a.webm");
        assert_eq!(uploadable_video(&app, webm.clone()).unwrap(), webm);
        let avi = dir.join("a.avi");
        let (back, e) = uploadable_video(&app, avi.clone()).unwrap_err();
        assert_eq!((back, e.code.as_str()), (avi, "unsupported_type"));
        assert!(e.message.contains("a.avi is not an MP4"), "{}", e.message);

        // A .mov without a usable ffmpeg: refused with the explanation.
        let mov = dir.join("a.mov");
        std::fs::write(&mov, b"qt").unwrap();
        app.state::<crate::settings::SettingsState>().0.lock().unwrap().ffmpeg_path = "/nonexistent/ffmpeg".into();
        let (back, e) = uploadable_video(&app, mov.clone()).unwrap_err();
        assert_eq!((back.clone(), e.code.as_str()), (mov.clone(), "unsupported_type"));
        assert!(e.message.contains("brew install ffmpeg"), "{}", e.message);
        assert!(mov.exists());

        // With one: converted, and the .mp4 is what goes up.
        let ffmpeg = fake_ffmpeg(&dir, true);
        app.state::<crate::settings::SettingsState>().0.lock().unwrap().ffmpeg_path = ffmpeg.to_string_lossy().into_owned();
        let converted = uploadable_video(&app, mov.clone()).unwrap();
        assert_eq!(converted, dir.join("a.mp4"));
        assert!(!mov.exists());

        // One that fails: the .mov stays and the failure is named.
        std::fs::write(&mov, b"qt").unwrap();
        let broken = fake_ffmpeg(&temp_dir("record-uploadable-broken"), false);
        app.state::<crate::settings::SettingsState>().0.lock().unwrap().ffmpeg_path = broken.to_string_lossy().into_owned();
        let (back, e) = uploadable_video(&app, mov.clone()).unwrap_err();
        assert_eq!((back, e.code.as_str()), (mov, "convert"));
    }

    /// Stop & upload on a finished `.mp4`: the file goes to the (local)
    /// share server, the link is copied and announced, the file stays and
    /// is not revealed (the bar shows "Link copied" for a moment instead).
    #[test]
    fn stop_and_upload_sends_the_recording_and_copies_the_link() {
        let dir = temp_dir("record-upload");
        let fake = share::tests::Fake::new(64, 1_000_000);
        let base = share::tests::serve_fake(&fake);
        let app = app_with(crate::settings::Settings { upload_server: base, ..settings_in(&dir) });
        share::reset(&app);
        let path = dir.join("clip.mp4");
        std::fs::write(&path, vec![7u8; 200]).unwrap();
        recording(&app, path.clone());

        assert_eq!(stop_with(&app, Outcome::Upload).unwrap(), Some(path.clone()));
        assert!(!is_recording(&app));
        assert!(path.exists(), "the recording stays on disk");
        let Some(share::Notice::Shared { link, .. }) = share::notice(&app) else { panic!("{:?}", share::notice(&app)) };
        assert_eq!(link.share_url, "https://socorin.com/s/med-0123456789abcdefgh");
        assert_eq!(app.state::<share::ShareState>().copied.lock().unwrap().as_slice(), &[link.share_url.clone()]);
        let init = &fake.requests("POST", "/api/upload/init")[0];
        let body: serde_json::Value = serde_json::from_slice(&init.body).unwrap();
        assert_eq!(body["kind"], "video");
        assert_eq!(body["mime"], "video/mp4");
        assert_eq!(body["size"], 200);
        assert_eq!(body["filename"], "clip.mp4");
        assert_eq!(fake.requests("PUT", "/api/upload/").len(), 4, "200 bytes in chunks of 64");
        assert_eq!(share::history(&app).len(), 1);
        share::dismiss(&app);
        std::thread::sleep(Duration::from_millis(30));
    }

    #[test]
    fn a_process_backend_finishes_on_the_stop_signal_or_is_killed() {
        // `sleep` dies of the stop signal at once.
        let started = Instant::now();
        assert_eq!(Backend::Process(sleeping_child()).finish(Duration::from_secs(5)), Ok(()));
        assert!(started.elapsed() < Duration::from_secs(4));
        Backend::Process(sleeping_child()).abandon();
        // One that ignores it is killed when the time is up. It says when
        // the handler is in place: a signal sent earlier would still end it.
        #[cfg(unix)]
        {
            let mut stubborn = std::process::Command::new("perl")
                .args(["-e", "$SIG{INT} = 'IGNORE'; $| = 1; print \"ready\\n\"; sleep 30"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("perl");
            {
                use std::io::{BufRead, BufReader};
                let mut line = String::new();
                BufReader::new(stubborn.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
                assert_eq!(line.trim(), "ready");
            }
            let started = Instant::now();
            let err = Backend::Process(stubborn).finish(Duration::from_millis(300)).unwrap_err();
            assert!(err.contains("killed"), "{err}");
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }
}
