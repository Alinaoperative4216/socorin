//! Tauri commands exposed to the webviews.

use serde::Serialize;
use tauri::{
    ipc::{InvokeBody, Request, Response},
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    audio,
    capture::{self, AppState, MonitorInfo, Region},
    debug,
    hotkey,
    record,
    settings::{self, Settings},
    share, update, windows,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: &'static str,
    pub screen_permission: bool,
    /// macOS: "granted" / "denied" / "undetermined" (never asked yet).
    /// Elsewhere "unknown": Windows and Linux have no question to read.
    pub mic_permission: &'static str,
    pub wayland: bool,
    /// macOS: "disk-image" / "translocated" when this copy of the app cannot
    /// be granted permissions (see `macos::install_issue`).
    pub install_issue: Option<&'static str>,
}

#[tauri::command]
pub fn platform_info() -> PlatformInfo {
    PlatformInfo {
        os: std::env::consts::OS,
        #[cfg(target_os = "macos")]
        screen_permission: crate::macos::has_screen_permission(),
        #[cfg(not(target_os = "macos"))]
        screen_permission: true,
        #[cfg(target_os = "macos")]
        mic_permission: crate::macos::mic_permission().as_str(),
        #[cfg(not(target_os = "macos"))]
        mic_permission: "unknown",
        wayland: cfg!(target_os = "linux")
            && std::env::var("XDG_SESSION_TYPE")
                .map(|v| v.eq_ignore_ascii_case("wayland"))
                .unwrap_or(false),
        #[cfg(target_os = "macos")]
        install_issue: crate::macos::install_issue(),
        #[cfg(not(target_os = "macos"))]
        install_issue: None,
    }
}

#[tauri::command]
pub fn request_screen_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::request_screen_permission()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[tauri::command]
pub fn start_capture<R: Runtime>(app: AppHandle<R>) {
    capture::begin_region(app);
}

#[tauri::command]
pub fn start_fullscreen_capture<R: Runtime>(app: AppHandle<R>) {
    capture::begin_fullscreen(app);
}

#[tauri::command]
pub fn cancel_capture<R: Runtime>(app: AppHandle<R>) {
    capture::cancel(&app);
}

/// Info for an overlay that loaded while a session is already active
/// (e.g. its window was just created). Errors when idle.
#[tauri::command]
pub fn overlay_info<R: Runtime>(app: AppHandle<R>, monitor_id: u32) -> Result<MonitorInfo, String> {
    let state = app.state::<AppState>();
    let shots = state.shots.lock().unwrap();
    shots
        .iter()
        .find(|s| s.geom.id == monitor_id)
        .map(|s| MonitorInfo {
            geom: s.geom.clone(),
            image_width: s.image.width(),
            image_height: s.image.height(),
            session: state.session.load(std::sync::atomic::Ordering::SeqCst),
            preselect_full: false,
            mode: capture::mode(&app),
        })
        .ok_or_else(|| "no capture session".into())
}

/// The overlay has painted its bitmap and can be shown.
#[tauri::command]
pub fn overlay_ready<R: Runtime>(app: AppHandle<R>, monitor_id: u32) {
    capture::overlay_ready(&app, monitor_id);
}

/// Raw RGBA8 pixels of a monitor bitmap (width/height come from `overlay_info`).
#[tauri::command]
pub fn overlay_pixels<R: Runtime>(app: AppHandle<R>, monitor_id: u32) -> Result<Response, String> {
    let state = app.state::<AppState>();
    let shots = state.shots.lock().unwrap();
    let shot = shots
        .iter()
        .find(|s| s.geom.id == monitor_id)
        .ok_or("capture session expired")?;
    Ok(Response::new(shot.image.as_raw().clone()))
}

/// `async` for the same reason as `edit_png`: in editor mode it opens the
/// editor window.
#[tauri::command(async)]
pub fn finish_region<R: Runtime>(app: AppHandle<R>, region: Region) -> Result<(), String> {
    capture::finish_region(&app, region)
}

/// The overlay has a selection and is annotating it in place.
#[tauri::command]
pub fn begin_annotation<R: Runtime>(app: AppHandle<R>, monitor_id: u32) {
    capture::begin_annotation(&app, monitor_id);
}

/// Body: PNG bytes. Ends the session and opens the image in the editor window.
/// `async` runs it off the main thread: on Windows, building a webview window
/// inside a synchronous command deadlocks WebView2 (a blank, frozen window;
/// see `WebviewWindowBuilder::new`).
#[tauri::command(async)]
pub fn edit_png<R: Runtime>(app: AppHandle<R>, request: Request<'_>) -> Result<(), String> {
    let png = raw_body(&request)?;
    capture::edit_png(&app, png)
}

/// Body: PNG bytes. Ends a running session and asks where to save through
/// the system dialog; resolves to the saved path, or `null` when the dialog
/// was cancelled. This is the only way a webview gets a PNG to a place of
/// its choosing: the dialog runs in Rust, no path crosses the IPC.
#[tauri::command]
pub async fn save_png_as<R: Runtime>(app: AppHandle<R>, request: Request<'_>) -> Result<Option<String>, String> {
    let png = raw_body(&request)?;
    let saved = capture::save_png_as(&app, png).await?;
    Ok(saved.map(|p| p.to_string_lossy().into_owned()))
}

/// PNG bytes of the image the editor should display.
#[tauri::command]
pub fn pending_image<R: Runtime>(app: AppHandle<R>) -> Result<Response, String> {
    let state = app.state::<AppState>();
    let pending = state.pending.lock().unwrap();
    pending
        .as_ref()
        .map(|png| Response::new(png.clone()))
        .ok_or_else(|| "no image".into())
}

fn raw_body(request: &Request<'_>) -> Result<Vec<u8>, String> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        InvokeBody::Json(_) => Err("expected a binary body".into()),
    }
}

/// The `x-socorin-anchor` header of an upload: the clicked button as JSON
/// `{ x, y, width, height }` in the page's CSS pixels, made absolute. A
/// missing or unreadable one only means the popover goes under the icon.
fn anchor_header<R: Runtime>(window: &tauri::Window<R>, request: &Request<'_>) -> Option<share::Anchor> {
    let raw = request.headers().get("x-socorin-anchor")?.to_str().ok()?;
    let anchor: share::Anchor = serde_json::from_str(raw).ok()?;
    windows::anchor_on_screen(window, anchor)
}

/// Body: PNG bytes, written to a new auto-named file in the save dir.
/// Returns the path. There is deliberately no way to name the destination:
/// a webview that could would be able to write a `.png` anywhere the user
/// can (see `save_png_as` for the dialog-driven way).
#[tauri::command]
pub fn save_png<R: Runtime>(app: AppHandle<R>, request: Request<'_>) -> Result<String, String> {
    let png = raw_body(&request)?;
    let settings = settings::current(&app);
    let saved = capture::save_png(&app, &png, None)?;
    debug::log(format!("saved {} ({} bytes)", saved.display(), png.len()));
    if settings.copy_on_save {
        capture::copy_png(&app, &png)?;
    }
    Ok(saved.to_string_lossy().into_owned())
}

/// Body: PNG bytes → system clipboard.
#[tauri::command]
pub fn copy_png<R: Runtime>(app: AppHandle<R>, request: Request<'_>) -> Result<(), String> {
    let png = raw_body(&request)?;
    capture::copy_png(&app, &png)
}

// ---- recording ----

/// Pick a region on the overlay and record it (stops a running recording).
#[tauri::command]
pub fn start_record_region<R: Runtime>(app: AppHandle<R>) {
    record::begin_region(app);
}

#[tauri::command]
pub fn start_record_fullscreen<R: Runtime>(app: AppHandle<R>) {
    record::begin_fullscreen(app);
}

/// The overlay chose the area to record (physical pixels of one monitor's
/// bitmap) and says where its Record bar is (`bar`, logical pixels relative
/// to the monitor), so the recording bar can take that exact place. The
/// session ends and the encoder starts on a worker thread; failures arrive
/// as `capture-error`.
#[tauri::command]
pub fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    region: Region,
    bar: Option<record::Bar>,
) -> Result<(), String> {
    let area = {
        let state = app.state::<AppState>();
        let shots = state.shots.lock().unwrap();
        let shot = shots
            .iter()
            .find(|s| s.geom.id == region.monitor_id)
            .ok_or("capture session expired")?;
        let s = (shot.geom.scale as f64).max(0.1);
        record::Area {
            monitor: shot.geom.clone(),
            x: region.x as f64 / s,
            y: region.y as f64 / s,
            width: region.width as f64 / s,
            height: region.height as f64 / s,
        }
    };
    std::thread::spawn(move || {
        if let Err(e) = record::start(&app, area, bar) {
            eprintln!("[record] {e}");
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

/// Stop and reveal the file.
#[tauri::command]
pub fn stop_recording<R: Runtime>(app: AppHandle<R>) {
    record::stop_async_with(app, record::Outcome::Reveal);
}

/// Stop and put the file on the clipboard.
#[tauri::command]
pub fn stop_recording_copy<R: Runtime>(app: AppHandle<R>) {
    record::stop_async_with(app, record::Outcome::Copy);
}

/// Stop, upload the file to the share server and put the link on the
/// clipboard (the popover reports the outcome).
/// `anchor`: the "Stop & upload" button in the bar's CSS pixels, where
/// the popover goes.
#[tauri::command]
pub fn stop_recording_upload<R: Runtime>(app: AppHandle<R>, window: tauri::Window<R>, anchor: Option<share::Anchor>) {
    share::set_anchor(&app, anchor.and_then(|a| windows::anchor_on_screen(&window, a)));
    record::stop_async_with(app, record::Outcome::Upload);
}

/// Stop and delete the file.
#[tauri::command]
pub fn cancel_recording<R: Runtime>(app: AppHandle<R>) {
    record::stop_async_with(app, record::Outcome::Discard);
}

#[tauri::command]
pub fn recording_status<R: Runtime>(app: AppHandle<R>) -> record::RecordingStatus {
    record::status(&app)
}

#[tauri::command]
pub fn get_settings<R: Runtime>(app: AppHandle<R>) -> Settings {
    settings::current(&app)
}

/// The settings a window other than the settings window may write. The
/// annotation style is saved by the editor and by the in-place editor on the
/// overlay as the user picks a colour or a stroke, and the overlay's Record
/// bar switches the microphone on and off; nothing else is any window's
/// business. The settings window (`windows::MAIN`) owns them all.
///
/// Without this, one `update_settings` call from any page could turn on the
/// command-line triggers, switch "after capture" to Upload or point
/// `uploadServer` somewhere else — settings that decide whether captures
/// leave the machine. (Which microphone is recorded stays with the settings
/// window too; the bar only says whether.)
const SHARED_KEYS: [&str; 3] = ["annotationColor", "annotationStroke", "mic"];

/// Partial update: only the keys present in `patch` change, so each window
/// can save the settings it owns without clobbering the others'. Returns the
/// resulting settings.
#[tauri::command]
pub fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    window: tauri::Window<R>,
    patch: serde_json::Map<String, serde_json::Value>,
) -> Result<Settings, String> {
    apply_settings(&app, window.label(), patch)
}

/// `update_settings` with the calling window named, which decides which
/// keys it may write.
pub(crate) fn apply_settings<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    patch: serde_json::Map<String, serde_json::Value>,
) -> Result<Settings, String> {
    if label != windows::MAIN {
        if let Some(key) = patch.keys().find(|k| !SHARED_KEYS.contains(&k.as_str())) {
            return Err(format!("{label} may not change {key}."));
        }
    }
    let previous = settings::current(app);
    let mut settings = settings::merge(&previous, &patch)?;
    settings::normalise(app, &mut settings);

    // The recorder runs whatever this names, so it has to be an ffmpeg.
    if patch.contains_key("ffmpegPath") && !settings.ffmpeg_path.is_empty() {
        record::configured_ffmpeg(&settings.ffmpeg_path)?;
    }

    // The share server: `normalise` quietly falls back to socorin.com, but
    // the settings window should hear that its entry was not an address.
    // An empty field means "the default", which is not a complaint.
    if let Some(value) = patch.get("uploadServer") {
        let raw = value.as_str().ok_or("uploadServer must be text")?;
        if !raw.trim().is_empty() && settings::sanitise_server_checked(raw).is_none() {
            return Err(format!(
                "The upload server must be an http(s) address such as {} or http://localhost:3000.",
                settings::DEFAULT_UPLOAD_SERVER
            ));
        }
    }

    // Validate hotkeys by registering them; roll back on failure. Only when
    // the settings window saved them: re-registering drops the global Escape
    // of a running capture session, and the overlay saves settings too.
    if patch.contains_key("hotkey") || patch.contains_key("fullscreenHotkey") {
        if let Err(e) = hotkey::apply(app, &settings) {
            let _ = hotkey::apply(app, &previous);
            return Err(e);
        }
    }

    if patch.contains_key("autostart") {
        let autolaunch = app.autolaunch();
        let result = if settings.autostart {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        if let Err(e) = result {
            eprintln!("[autostart] {e}");
        }
    }

    settings::store(app, settings.clone())?;
    Ok(settings)
}

/// The welcome page has rendered: show (and focus) its window.
#[tauri::command]
pub fn welcome_ready<R: Runtime>(app: AppHandle<R>) {
    windows::show_welcome(&app);
}

/// OK on the welcome dialog: remember it and close the window.
#[tauri::command]
pub fn dismiss_welcome<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    settings::mark_welcome_shown(&app)?;
    windows::close_welcome(&app);
    Ok(())
}

#[tauri::command]
pub fn debug_options() -> debug::DebugOptions {
    debug::options()
}

/// Lets the webviews add lines to the diagnostics log.
#[tauri::command]
pub fn debug_log(message: String) {
    debug::log(format!("[ui] {message}"));
}

#[tauri::command]
pub fn default_save_dir<R: Runtime>(app: AppHandle<R>) -> String {
    settings::default_save_dir(&app).to_string_lossy().into_owned()
}

/// Opens the OS folder where quick-saved screenshots go (creating it if needed).
#[tauri::command]
pub fn open_save_dir<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let dir = settings::save_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(dir, None::<&str>).map_err(|e| e.to_string())
}

/// The microphones this computer has right now (Settings → Recording's
/// list, and the Record bar's tooltip). Never prompts.
#[tauri::command]
pub fn audio_inputs() -> Vec<audio::Input> {
    audio::inputs()
}

/// macOS: jump straight to Privacy & Security → Microphone.
#[tauri::command]
pub fn open_mic_permission_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        tauri_plugin_opener::open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// macOS: jump straight to Privacy & Security → Screen Recording.
#[tauri::command]
pub fn open_screen_permission_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        tauri_plugin_opener::open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[tauri::command]
pub fn show_settings<R: Runtime>(app: AppHandle<R>) {
    windows::show_settings(&app);
}

// ---- updates ----

#[tauri::command]
pub fn update_status<R: Runtime>(app: AppHandle<R>) -> update::Status {
    update::status(&app)
}

/// Settings → "Check now": look right away, whatever the schedule says.
/// Async, so the request never blocks the main thread.
#[tauri::command]
pub async fn check_for_updates<R: Runtime>(app: AppHandle<R>) -> Result<update::Status, String> {
    update::check(app.clone(), update::version_url()).await?;
    Ok(update::status(&app))
}

/// "Update now" in the popover or in Settings.
#[tauri::command]
pub fn install_update<R: Runtime>(app: AppHandle<R>) {
    update::install(&app);
}

/// "Later" / OK / Escape in the popover.
#[tauri::command]
pub fn dismiss_update<R: Runtime>(app: AppHandle<R>) {
    update::dismiss(&app);
}

/// The popover has rendered: show its window.
#[tauri::command]
pub fn update_ready<R: Runtime>(app: AppHandle<R>) {
    windows::reveal_update_notice(&app);
}

// ---- sharing ----

/// Body: PNG bytes; header `x-socorin-anchor`: the clicked button (see
/// `anchor_header`), where the popover goes. Uploads it to the share
/// server, puts the link on the clipboard and shows the popover. Resolves
/// to the link without its delete token (a `PublicLink`, as the history
/// and the popover get it); a failure comes back as a `ShareError`
/// (`{ code, message, … }`) for the page's toast.
#[tauri::command]
pub async fn upload_png<R: Runtime>(
    app: AppHandle<R>,
    window: tauri::Window<R>,
    request: Request<'_>,
) -> Result<share::PublicLink, share::ShareError> {
    let png = raw_body(&request).map_err(|e| share::ShareError::new("invalid_request", e))?;
    share::share_png(&app, png, anchor_header(&window, &request)).await
}

/// The remembered links, newest first (without their delete tokens). The
/// file is tidied up at the same time: links the server has already dropped
/// go, and with them their delete tokens.
#[tauri::command]
pub fn share_history<R: Runtime>(app: AppHandle<R>) -> Vec<share::PublicLink> {
    share::prune_history(&app);
    share::history(&app).iter().map(share::SharedLink::public).collect()
}

/// "Delete from server" for a remembered link.
#[tauri::command]
pub async fn delete_share<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), share::ShareError> {
    share::delete(&app, &id).await
}

/// Put a remembered link on the clipboard again.
#[tauri::command]
pub fn copy_share_link<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    share::copy_link(&app, &id)
}

/// Settings → "Reset install ID": the next upload registers a new one.
#[tauri::command]
pub fn reset_install_id<R: Runtime>(app: AppHandle<R>) -> Result<Settings, String> {
    let mut settings = settings::current(&app);
    settings.install_id.clear();
    settings::store(&app, settings.clone())?;
    Ok(settings)
}

/// What the "Link copied" popover shows.
#[tauri::command]
pub fn share_notice<R: Runtime>(app: AppHandle<R>) -> Option<share::Notice> {
    share::notice(&app)
}

/// The popover has rendered: show its window.
#[tauri::command]
pub fn share_ready<R: Runtime>(app: AppHandle<R>) {
    windows::reveal_share_notice(&app);
}

/// Close / Escape / the timer in the popover.
#[tauri::command]
pub fn dismiss_share<R: Runtime>(app: AppHandle<R>) {
    share::dismiss(&app);
}

/// The user clicked into the popover: a click elsewhere closes it from
/// now on.
#[tauri::command]
pub fn share_engaged<R: Runtime>(app: AppHandle<R>) {
    share::engaged(&app);
}

#[tauri::command]
pub fn quit<R: Runtime>(app: AppHandle<R>) {
    record::stop_if_recording(&app);
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        settings::AfterCapture,
        test_support::{app_with, geom, settings_in, shot, start_session, temp_dir},
    };
    use serde_json::json;
    use tauri::{
        ipc::{CallbackFn, InvokeBody, InvokeResponseBody},
        test::{get_ipc_response, MockRuntime},
        webview::InvokeRequest,
        AppHandle, WebviewUrl, WebviewWindowBuilder,
    };

    fn patch(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    /// Run a command through the real IPC layer (raw bodies and headers
    /// included), as the webview would, from a window with no special
    /// rights (see `invoke_from` for one that has them).
    fn invoke(
        app: &AppHandle<MockRuntime>,
        cmd: &str,
        body: InvokeBody,
        headers: &[(&str, &str)],
    ) -> Result<InvokeResponseBody, serde_json::Value> {
        invoke_from(app, "ipc", cmd, body, headers)
    }

    /// The same, from the window named by `label`: which window a command
    /// comes from decides what `update_settings` may write.
    fn invoke_from(
        app: &AppHandle<MockRuntime>,
        label: &str,
        cmd: &str,
        body: InvokeBody,
        headers: &[(&str, &str)],
    ) -> Result<InvokeResponseBody, serde_json::Value> {
        let webview = match app.get_webview_window(label) {
            Some(w) => w,
            None => WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
                .visible(false)
                .build()
                .unwrap(),
        };
        let mut map = tauri::http::HeaderMap::new();
        for (k, v) in headers {
            map.insert(
                tauri::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        get_ipc_response(
            &webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body,
                headers: map,
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    }

    #[test]
    fn platform_info_describes_this_machine() {
        let info = platform_info();
        assert_eq!(info.os, std::env::consts::OS);
        assert!(!info.wayland);
        assert_eq!(info.install_issue, None);
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("screenPermission").is_some());
        let mic = json["micPermission"].as_str().unwrap();
        if cfg!(target_os = "macos") {
            assert!(["granted", "denied", "undetermined"].contains(&mic), "{mic}");
        } else {
            assert_eq!(mic, "unknown");
        }
    }

    /// The microphone list is the platform's, through the IPC layer too;
    /// the overlay may switch the microphone on or off, but only the
    /// settings window may say which one.
    #[test]
    fn microphones_are_listed_and_only_the_switch_is_shared() {
        let dir = temp_dir("cmd-mic");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        let listed = audio_inputs();
        assert_eq!(listed, audio::inputs());
        let res = invoke(&handle, "audio_inputs", InvokeBody::default(), &[]).unwrap();
        let json = res.deserialize::<serde_json::Value>().unwrap();
        assert_eq!(json.as_array().map(|a| a.len()), Some(listed.len()));

        let updated = apply_settings(&handle, "overlay-1", patch(json!({ "mic": false }))).unwrap();
        assert!(!updated.mic);
        assert!(!get_settings(handle.clone()).mic);
        let err = apply_settings(&handle, "overlay-1", patch(json!({ "micDevice": "usb:1" }))).unwrap_err();
        assert!(err.contains("may not change micDevice"), "{err}");
        let updated = apply_settings(
            &handle,
            windows::MAIN,
            patch(json!({ "mic": true, "micDevice": " usb:1 ", "micDeviceName": "Jabra" })),
        )
        .unwrap();
        assert!(updated.mic);
        assert_eq!((updated.mic_device.as_str(), updated.mic_device_name.as_str()), ("usb:1", "Jabra"));
    }

    #[test]
    fn overlay_queries_need_a_live_session() {
        let app = app_with(settings_in(&temp_dir("cmd-overlay")));
        let handle = app.handle().clone();
        assert_eq!(overlay_info(handle.clone(), 1).unwrap_err(), "no capture session");
        assert!(overlay_pixels(handle.clone(), 1).is_err());
        assert_eq!(pending_image(handle.clone()).err(), Some("no image".to_string()));

        let session = start_session(&app, vec![shot(geom(1, 0, 0, 20, 10, 2.0, true))]);
        let info = overlay_info(handle.clone(), 1).unwrap();
        assert_eq!((info.image_width, info.image_height, info.session), (40, 20, session));
        assert_eq!(info.mode, capture::Mode::Screenshot);
        assert!(!info.preselect_full);
        assert!(overlay_pixels(handle.clone(), 1).is_ok());
        overlay_ready(handle.clone(), 1);
        begin_annotation(handle.clone(), 99); // unknown monitor: harmless
        assert!(start_recording(handle.clone(), Region { monitor_id: 5, x: 0, y: 0, width: 4, height: 4 }, None).is_err());

        finish_region(handle.clone(), Region { monitor_id: 1, x: 0, y: 0, width: 10, height: 10 }).unwrap();
        assert!(pending_image(handle.clone()).is_ok());
        cancel_capture(handle);
    }

    #[test]
    fn capture_and_record_entry_points_are_guarded_while_busy() {
        let app = app_with(settings_in(&temp_dir("cmd-entry")));
        let handle = app.handle().clone();
        let session = start_session(&app, vec![shot(geom(1, 0, 0, 8, 8, 1.0, true))]);
        start_capture(handle.clone());
        start_fullscreen_capture(handle.clone());
        start_record_region(handle.clone());
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(handle.state::<AppState>().session.load(std::sync::atomic::Ordering::SeqCst), session);
        cancel_capture(handle.clone());
        assert!(!capture::is_busy(&handle));

        *handle.state::<record::RecordState>().active.lock().unwrap() = Some(record::Active {
            backend: record::Backend::Process(crate::test_support::sleeping_child()),
            path: std::path::PathBuf::from("/nonexistent/x.mov"),
            started: std::time::Instant::now(),
            started_ms: 1,
            audio: None,
            audio_issue: None,
        });
        assert!(recording_status(handle.clone()).recording);
        start_record_fullscreen(handle.clone()); // toggles: stops the recording
        for _ in 0..100 {
            if !recording_status(handle.clone()).recording {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!recording_status(handle).recording);
    }

    #[test]
    fn png_commands_take_raw_bodies_and_never_a_path() {
        let dir = temp_dir("cmd-png");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();

        let err = invoke(&handle, "save_png", InvokeBody::Json(json!({})), &[]).unwrap_err();
        assert_eq!(err, json!("expected a binary body"));

        let saved = invoke(&handle, "save_png", InvokeBody::Raw(b"png".to_vec()), &[]).unwrap();
        let path: String = saved.deserialize().unwrap();
        assert!(path.starts_with(dir.to_str().unwrap()));
        assert_eq!(std::fs::read(&path).unwrap(), b"png");

        // A webview naming the destination (as an older frontend did through
        // `x-path`) is ignored: the file still lands in the save folder.
        let elsewhere = temp_dir("cmd-png-elsewhere").join("planted.png");
        let saved = invoke(&handle, "save_png", InvokeBody::Raw(b"two".to_vec()), &[("x-path", elsewhere.to_str().unwrap())]).unwrap();
        let path: String = saved.deserialize().unwrap();
        assert!(path.starts_with(dir.to_str().unwrap()));
        assert!(!elsewhere.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"two");

        let err = invoke(&handle, "copy_png", InvokeBody::Raw(b"nope".to_vec()), &[]).unwrap_err();
        assert!(err.as_str().unwrap().contains("Unsupported") || !err.as_str().unwrap().is_empty());

        invoke(&handle, "edit_png", InvokeBody::Raw(b"\x89PNG".to_vec()), &[]).unwrap();
        assert!(pending_image(handle.clone()).is_ok());
        assert!(invoke(&handle, "edit_png", InvokeBody::Json(json!(1)), &[]).is_err());
        assert!(invoke(&handle, "save_png_as", InvokeBody::Json(json!(1)), &[]).is_err());
    }

    #[test]
    fn settings_commands_merge_validate_and_persist() {
        let dir = temp_dir("cmd-settings");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        assert_eq!(get_settings(handle.clone()).save_dir, dir.to_string_lossy());
        assert_eq!(default_save_dir(handle.clone()), settings::default_save_dir(&handle).to_string_lossy());

        let updated = apply_settings(
            &handle,
            windows::MAIN,
            patch(json!({ "afterCapture": "save", "filePrefix": " Shot ", "annotationStroke": 9 })),
        )
        .unwrap();
        assert_eq!(updated.after_capture, AfterCapture::Save);
        assert_eq!(updated.file_prefix, "Shot");
        assert_eq!(updated.annotation_stroke, 5);
        assert_eq!(get_settings(handle.clone()).file_prefix, "Shot");

        // Hotkeys are validated by registering them; a bad one is rolled back.
        let err = apply_settings(&handle, windows::MAIN, patch(json!({ "hotkey": "Nope+" }))).unwrap_err();
        assert!(err.contains("cannot register"), "{err}");
        assert_eq!(get_settings(handle.clone()).hotkey, "CmdOrCtrl+Shift+A");
        apply_settings(&handle, windows::MAIN, patch(json!({ "hotkey": "Ctrl+Alt+Shift+F16", "fullscreenHotkey": "" }))).unwrap();
        assert_eq!(get_settings(handle.clone()).hotkey, "Ctrl+Alt+Shift+F16");

        // Autostart writes / removes a launch agent under the (private) home.
        apply_settings(&handle, windows::MAIN, patch(json!({ "autostart": true }))).unwrap();
        apply_settings(&handle, windows::MAIN, patch(json!({ "autostart": false }))).unwrap();
        assert!(!get_settings(handle.clone()).autostart);

        assert!(apply_settings(&handle, windows::MAIN, patch(json!({ "copyOnSave": "yes" }))).is_err());

        // The recorder runs the configured binary, so only an existing
        // `ffmpeg` is accepted; empty means "search for it".
        let fake = dir.join("ffmpeg");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        let not_ffmpeg = dir.join("other");
        std::fs::write(&not_ffmpeg, b"#!/bin/sh\n").unwrap();
        let err = apply_settings(&handle, windows::MAIN, patch(json!({ "ffmpegPath": not_ffmpeg.to_str().unwrap() }))).unwrap_err();
        assert!(err.contains("not an ffmpeg"), "{err}");
        let err = apply_settings(&handle, windows::MAIN, patch(json!({ "ffmpegPath": dir.join("missing").join("ffmpeg").to_str().unwrap() }))).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert_eq!(get_settings(handle.clone()).ffmpeg_path, "");
        let updated = apply_settings(&handle, windows::MAIN, patch(json!({ "ffmpegPath": fake.to_str().unwrap() }))).unwrap();
        assert_eq!(updated.ffmpeg_path, fake.to_string_lossy());
        apply_settings(&handle, windows::MAIN, patch(json!({ "ffmpegPath": " " }))).unwrap();
        assert_eq!(get_settings(handle.clone()).ffmpeg_path, "");

        // Through the IPC layer as well.
        let res = invoke(&handle, "get_settings", InvokeBody::default(), &[]).unwrap();
        assert!(res.deserialize::<settings::Settings>().is_ok());
        let res = invoke_from(&handle, windows::MAIN, "update_settings", InvokeBody::Json(json!({ "patch": { "filePrefix": "Ipc" } })), &[]).unwrap();
        assert_eq!(res.deserialize::<settings::Settings>().unwrap().file_prefix, "Ipc");
        apply_settings(&handle, windows::MAIN, patch(json!({ "hotkey": "" }))).unwrap();
    }

    #[test]
    fn welcome_recording_and_diagnostics_commands() {
        let dir = temp_dir("cmd-misc");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        dismiss_welcome(handle.clone()).unwrap();
        assert!(get_settings(handle.clone()).welcome_shown);

        assert!(!recording_status(handle.clone()).recording);
        stop_recording(handle.clone());
        stop_recording_copy(handle.clone());
        cancel_recording(handle.clone());
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(!recording_status(handle.clone()).recording);

        let opts = debug_options();
        assert_eq!(opts.enabled, crate::debug::options().enabled);
        debug_log("hello".into());
        show_settings(handle.clone());
        assert!(request_screen_permission_is_platform_specific());
        // Not on macOS there is nothing to open; on macOS the URL scheme is
        // handed to the system (no window, no prompt).
        if !cfg!(target_os = "macos") {
            assert_eq!(open_mic_permission_settings(), Ok(()));
        }
        let res = invoke(&handle, "platform_info", InvokeBody::default(), &[]).unwrap();
        assert!(res.deserialize::<serde_json::Value>().unwrap().get("os").is_some());
    }

    /// `check_for_updates` and `install_update` reach socorin.com; the
    /// update module tests them against a local server instead.
    #[test]
    fn update_commands_report_and_dismiss() {
        let dir = temp_dir("cmd-update");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        let status = update_status(handle.clone());
        assert_eq!(status.available, "");
        assert_eq!(status.current_version, handle.package_info().version.to_string());
        let res = invoke(&handle, "update_status", InvokeBody::default(), &[]).unwrap();
        let json = res.deserialize::<serde_json::Value>().unwrap();
        assert_eq!(json["phase"]["phase"], "idle");
        assert_eq!(json["justUpdated"], false);
        update_ready(handle.clone()); // no popover window: nothing to show
        dismiss_update(handle.clone());
        assert_eq!(update_status(handle).phase, update::Phase::Idle);
    }

    /// D-9: which window a save comes from decides what it may write.
    /// Only the settings window owns the settings that decide whether
    /// captures leave the machine.
    #[test]
    fn only_the_settings_window_may_change_more_than_the_annotation_style() {
        let dir = temp_dir("cmd-settings-scope");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();

        // The editor and the overlay save the style as the user picks it.
        for label in [windows::EDITOR, "overlay-1"] {
            let saved = apply_settings(&handle, label, patch(json!({ "annotationColor": "#00ff00", "annotationStroke": 4 }))).unwrap();
            assert_eq!((saved.annotation_color.as_str(), saved.annotation_stroke), ("#00ff00", 4));
        }

        // Nothing else, whichever window asks. These are exactly the
        // settings that would let one page send every capture elsewhere.
        for key in ["cliTriggers", "afterCapture", "uploadServer", "installId", "saveDir", "hotkey", "autostart"] {
            for label in [windows::EDITOR, windows::SHARE, "overlay-1", "ipc"] {
                let err = apply_settings(&handle, label, patch(json!({ key: "x" }))).unwrap_err();
                assert!(err.contains(key) && err.contains(label), "{label}/{key}: {err}");
            }
        }
        // A mixed patch is refused whole: the style does not smuggle the rest in.
        assert!(apply_settings(&handle, windows::EDITOR, patch(json!({ "annotationColor": "#111111", "cliTriggers": true }))).is_err());
        assert!(!get_settings(handle.clone()).cli_triggers);
        assert_eq!(get_settings(handle.clone()).upload_server, settings::DEFAULT_UPLOAD_SERVER);

        // The settings window may, and the check runs over the real IPC too.
        assert!(apply_settings(&handle, windows::MAIN, patch(json!({ "cliTriggers": true }))).unwrap().cli_triggers);
        let err = invoke(&handle, "update_settings", InvokeBody::Json(json!({ "patch": { "uploadServer": "http://attacker.test" } }))
            , &[]).unwrap_err();
        assert!(err.as_str().unwrap_or_default().contains("may not change uploadServer"), "{err}");
        assert_eq!(get_settings(handle.clone()).upload_server, settings::DEFAULT_UPLOAD_SERVER);
        apply_settings(&handle, windows::MAIN, patch(json!({ "cliTriggers": false }))).unwrap();
    }

    #[test]
    fn the_upload_server_setting_is_validated_when_saved() {
        let dir = temp_dir("cmd-share-settings");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        let updated = apply_settings(&handle, windows::MAIN, patch(json!({ "uploadServer": "http://localhost:3000/" }))).unwrap();
        assert_eq!(updated.upload_server, "http://localhost:3000");
        // Empty, or the default with a slash: back to socorin.com, no complaint.
        assert_eq!(apply_settings(&handle, windows::MAIN, patch(json!({ "uploadServer": " " }))).unwrap().upload_server, settings::DEFAULT_UPLOAD_SERVER);
        assert_eq!(apply_settings(&handle, windows::MAIN, patch(json!({ "uploadServer": "https://socorin.com/" }))).unwrap().upload_server, settings::DEFAULT_UPLOAD_SERVER);
        for bad in ["socorin.com", "ftp://x", "https://socorin.com/api", "nope"] {
            let err = apply_settings(&handle, windows::MAIN, patch(json!({ "uploadServer": bad }))).unwrap_err();
            assert!(err.contains("http(s) address"), "{bad}: {err}");
        }
        assert_eq!(get_settings(handle.clone()).upload_server, settings::DEFAULT_UPLOAD_SERVER);

        // The install id is Rust's: reset empties it.
        *handle.state::<settings::SettingsState>().0.lock().unwrap() = Settings { install_id: "abc".into(), ..settings::current(&handle) };
        assert_eq!(reset_install_id(handle.clone()).unwrap().install_id, "");
        assert_eq!(get_settings(handle.clone()).install_id, "");
    }

    /// D-5: what the page is handed after an upload carries no delete
    /// token — it is the same `PublicLink` the history and the popover
    /// get, and deleting goes by id through `delete_share`.
    #[test]
    fn upload_png_hands_the_page_no_delete_token() {
        let dir = temp_dir("cmd-upload-token");
        let fake = share::tests::Fake::new(share::tests::TEST_CHUNK, 5_000_000);
        let base = share::tests::serve_fake(&fake);
        let app = app_with(settings::Settings { upload_server: base, ..settings_in(&dir) });
        let handle = app.handle().clone();
        share::reset(&handle);

        let png = b"\x89PNG\r\n\x1a\n0123456789".to_vec();
        // The clicked button rides along as a header: the popover goes
        // right under it (the mock window sits at the origin, scale 1).
        let anchor = r#"{"x":400,"y":50,"width":80,"height":30}"#;
        let res = invoke(&handle, "upload_png", InvokeBody::Raw(png), &[("x-socorin-anchor", anchor)]).unwrap();
        let (w, _) = windows::SHARE_SIZE;
        assert_eq!(*handle.state::<share::ShareState>().placed.lock().unwrap(), Some((440.0 - w / 2.0, 80.0)));
        let value: serde_json::Value = res.deserialize().unwrap();
        assert_eq!(value["shareUrl"], fake.share_url());
        assert!(value.get("deleteToken").is_none(), "the token must not cross the IPC: {value}");

        // Rust still has it, and the history the page reads has not either.
        assert_eq!(share::history(&handle)[0].delete_token.len(), 43);
        let listed = invoke(&handle, "share_history", InvokeBody::default(), &[]).unwrap();
        let listed: serde_json::Value = listed.deserialize().unwrap();
        assert!(listed[0].get("deleteToken").is_none(), "{listed}");
        // And the id is enough to take the file down again.
        tauri::async_runtime::block_on(delete_share(handle.clone(), value["id"].as_str().unwrap().into())).unwrap();
        assert_eq!(fake.deleted().len(), 1);
        share::dismiss(&handle);
    }

    /// The upload itself is tested in `share` against a local server; here
    /// the commands are reached through the IPC layer with a body that
    /// cannot go anywhere (no server listening).
    #[test]
    fn share_commands_take_raw_bodies_and_report_share_errors() {
        let dir = temp_dir("cmd-share");
        let app = app_with(Settings { upload_server: "http://127.0.0.1:9".into(), ..settings_in(&dir) });
        let handle = app.handle().clone();
        share::reset(&handle);
        let err = invoke(&handle, "upload_png", InvokeBody::Json(json!({})), &[]).unwrap_err();
        assert_eq!(err["code"], "invalid_request");
        let err = invoke(&handle, "upload_png", InvokeBody::Raw(b"\x89PNG".to_vec()), &[("x-socorin-anchor", "nope")]).unwrap_err();
        assert_eq!(err["code"], "network");
        // An unreadable anchor is no reason to fail: the popover goes under the icon.
        assert_eq!(*handle.state::<share::ShareState>().placed.lock().unwrap(), Some((windows::UPDATE_MARGIN, windows::UPDATE_MARGIN)));
        assert!(err["message"].as_str().unwrap().contains("127.0.0.1:9"));
        assert!(matches!(share_notice(handle.clone()), Some(share::Notice::Failed { .. })));

        assert!(share_history(handle.clone()).is_empty());
        let err = tauri::async_runtime::block_on(delete_share(handle.clone(), "nope".into())).unwrap_err();
        assert_eq!(err.code, "not_found");
        assert!(copy_share_link(handle.clone(), "nope".into()).is_err());
        share_ready(handle.clone());
        dismiss_share(handle.clone());
        // Not recording: logged. The button's place is kept for the popover
        // of the stop it would have started, relative to the calling window.
        let anchor = json!({ "anchor": { "x": 1, "y": 2, "width": 3, "height": 4 } });
        invoke(&handle, "stop_recording_upload", InvokeBody::Json(anchor), &[]).unwrap();
        assert_eq!(share::take_anchor(&handle), Some(share::Anchor { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }));
        invoke(&handle, "stop_recording_upload", InvokeBody::Json(json!({})), &[]).unwrap();
        assert_eq!(share::take_anchor(&handle), None);
        invoke(&handle, "share_engaged", InvokeBody::default(), &[]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let res = invoke(&handle, "share_history", InvokeBody::default(), &[]).unwrap();
        assert_eq!(res.deserialize::<Vec<share::PublicLink>>().unwrap(), vec![]);
    }

    /// `request_screen_permission` would show the macOS prompt; only its
    /// non-macOS branch is exercised here.
    fn request_screen_permission_is_platform_specific() -> bool {
        if cfg!(target_os = "macos") {
            true
        } else {
            request_screen_permission()
        }
    }
}
