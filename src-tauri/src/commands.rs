//! Tauri commands exposed to the webviews.

use serde::Serialize;
use tauri::{
    ipc::{InvokeBody, Request, Response},
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    capture::{self, AppState, MonitorInfo, Region},
    debug,
    hotkey,
    record,
    settings::{self, Settings},
    update, windows,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: &'static str,
    pub screen_permission: bool,
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

#[tauri::command]
pub fn finish_region<R: Runtime>(app: AppHandle<R>, region: Region) -> Result<(), String> {
    capture::finish_region(&app, region)
}

/// The overlay has a selection and is annotating it in place.
#[tauri::command]
pub fn begin_annotation<R: Runtime>(app: AppHandle<R>, monitor_id: u32) {
    capture::begin_annotation(&app, monitor_id);
}

/// Body: PNG bytes. Ends the session and opens the image in the editor window.
#[tauri::command]
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

/// Partial update: only the keys present in `patch` change, so each window
/// can save the settings it owns without clobbering the others'. Returns the
/// resulting settings.
#[tauri::command]
pub fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    patch: serde_json::Map<String, serde_json::Value>,
) -> Result<Settings, String> {
    let previous = settings::current(&app);
    let mut settings = settings::merge(&previous, &patch)?;
    settings::normalise(&app, &mut settings);

    // The recorder runs whatever this names, so it has to be an ffmpeg.
    if patch.contains_key("ffmpegPath") && !settings.ffmpeg_path.is_empty() {
        record::configured_ffmpeg(&settings.ffmpeg_path)?;
    }

    // Validate hotkeys by registering them; roll back on failure. Only when
    // the settings window saved them: re-registering drops the global Escape
    // of a running capture session, and the overlay saves settings too.
    if patch.contains_key("hotkey") || patch.contains_key("fullscreenHotkey") {
        if let Err(e) = hotkey::apply(&app, &settings) {
            let _ = hotkey::apply(&app, &previous);
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

    settings::store(&app, settings.clone())?;
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
    /// included), as the webview would.
    fn invoke(
        app: &AppHandle<MockRuntime>,
        cmd: &str,
        body: InvokeBody,
        headers: &[(&str, &str)],
    ) -> Result<InvokeResponseBody, serde_json::Value> {
        let webview = match app.get_webview_window("ipc") {
            Some(w) => w,
            None => WebviewWindowBuilder::new(app, "ipc", WebviewUrl::App("index.html".into()))
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

        let updated = update_settings(
            handle.clone(),
            patch(json!({ "afterCapture": "save", "filePrefix": " Shot ", "annotationStroke": 9 })),
        )
        .unwrap();
        assert_eq!(updated.after_capture, AfterCapture::Save);
        assert_eq!(updated.file_prefix, "Shot");
        assert_eq!(updated.annotation_stroke, 5);
        assert_eq!(get_settings(handle.clone()).file_prefix, "Shot");

        // Hotkeys are validated by registering them; a bad one is rolled back.
        let err = update_settings(handle.clone(), patch(json!({ "hotkey": "Nope+" }))).unwrap_err();
        assert!(err.contains("cannot register"), "{err}");
        assert_eq!(get_settings(handle.clone()).hotkey, "CmdOrCtrl+Shift+A");
        update_settings(handle.clone(), patch(json!({ "hotkey": "Ctrl+Alt+Shift+F16", "fullscreenHotkey": "" }))).unwrap();
        assert_eq!(get_settings(handle.clone()).hotkey, "Ctrl+Alt+Shift+F16");

        // Autostart writes / removes a launch agent under the (private) home.
        update_settings(handle.clone(), patch(json!({ "autostart": true }))).unwrap();
        update_settings(handle.clone(), patch(json!({ "autostart": false }))).unwrap();
        assert!(!get_settings(handle.clone()).autostart);

        assert!(update_settings(handle.clone(), patch(json!({ "copyOnSave": "yes" }))).is_err());

        // The recorder runs the configured binary, so only an existing
        // `ffmpeg` is accepted; empty means "search for it".
        let fake = dir.join("ffmpeg");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        let not_ffmpeg = dir.join("other");
        std::fs::write(&not_ffmpeg, b"#!/bin/sh\n").unwrap();
        let err = update_settings(handle.clone(), patch(json!({ "ffmpegPath": not_ffmpeg.to_str().unwrap() }))).unwrap_err();
        assert!(err.contains("not an ffmpeg"), "{err}");
        let err = update_settings(handle.clone(), patch(json!({ "ffmpegPath": dir.join("missing").join("ffmpeg").to_str().unwrap() }))).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert_eq!(get_settings(handle.clone()).ffmpeg_path, "");
        let updated = update_settings(handle.clone(), patch(json!({ "ffmpegPath": fake.to_str().unwrap() }))).unwrap();
        assert_eq!(updated.ffmpeg_path, fake.to_string_lossy());
        update_settings(handle.clone(), patch(json!({ "ffmpegPath": " " }))).unwrap();
        assert_eq!(get_settings(handle.clone()).ffmpeg_path, "");

        // Through the IPC layer as well.
        let res = invoke(&handle, "get_settings", InvokeBody::default(), &[]).unwrap();
        assert!(res.deserialize::<settings::Settings>().is_ok());
        let res = invoke(&handle, "update_settings", InvokeBody::Json(json!({ "patch": { "filePrefix": "Ipc" } })), &[]).unwrap();
        assert_eq!(res.deserialize::<settings::Settings>().unwrap().file_prefix, "Ipc");
        update_settings(handle, patch(json!({ "hotkey": "" }))).unwrap();
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
