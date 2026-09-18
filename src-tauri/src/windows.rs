//! Window management: overlay windows (one per monitor, created once and kept
//! hidden between captures), the editor and the settings window.

use std::collections::HashSet;

use tauri::{
    AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, Monitor, Position, Rect,
    Runtime, Size, WebviewUrl, WebviewWindowBuilder,
};

use crate::{
    capture::{self, MonitorGeom, MonitorInfo},
    debug, record, share,
};


pub const OVERLAY_PREFIX: &str = "overlay-";
pub const EDITOR: &str = "editor";
pub const MAIN: &str = "main";
pub const WELCOME: &str = "welcome";
pub const RECORDER: &str = "recorder";
pub const UPDATE: &str = "update";
pub const SHARE: &str = "share";

pub fn overlay_label(monitor_id: u32) -> String {
    format!("{OVERLAY_PREFIX}{monitor_id}")
}

/// Make sure exactly one hidden overlay window exists per monitor, positioned
/// over it. Existing windows are reused (that is what makes a capture fast);
/// windows for unplugged monitors are dropped.
pub fn ensure_overlays<R: Runtime>(app: &AppHandle<R>, monitors: &[MonitorGeom]) -> Result<(), String> {
    let wanted: HashSet<String> = monitors.iter().map(|m| overlay_label(m.id)).collect();
    for (label, window) in app.webview_windows() {
        if label.starts_with(OVERLAY_PREFIX) && !wanted.contains(&label) {
            debug::log(format!("dropping stale {label}"));
            let _ = window.destroy();
        }
    }

    for m in monitors {
        let label = overlay_label(m.id);
        if let Some(window) = app.get_webview_window(&label) {
            // Keep geometry in sync in case the display arrangement changed.
            let _ = window.set_position(LogicalPosition::new(m.x as f64, m.y as f64));
            let _ = window.set_size(LogicalSize::new(m.width as f64, m.height as f64));
            continue;
        }

        let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title("Capture")
            .decorations(false)
            .resizable(false)
            .minimizable(false)
            .maximizable(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .shadow(false)
            .accept_first_mouse(true)
            .focused(false)
            .visible(false)
            .position(m.x as f64, m.y as f64)
            .inner_size(m.width as f64, m.height as f64)
            .background_color(tauri::window::Color(0, 0, 0, 255));
        // Wayland compositors ignore client-side positioning; with a single
        // monitor a fullscreen window lands in the right place anyway.
        #[cfg(target_os = "linux")]
        let builder = if monitors.len() == 1 && is_wayland() {
            builder.fullscreen(true)
        } else {
            builder
        };
        let window = builder
            .build()
            .map_err(|e| format!("cannot create overlay window: {e}"))?;
        debug::log(format!("created {label} at ({}, {}) {}x{}", m.x, m.y, m.width, m.height));
        // The test runtime has no NSWindow behind its windows.
        #[cfg(all(target_os = "macos", not(test)))]
        crate::macos::raise_overlay(&window);
        #[cfg(any(not(target_os = "macos"), test))]
        let _ = &window;
    }
    Ok(())
}

/// Create the hidden overlay windows ahead of time so the first capture is
/// as fast as the following ones.
pub fn prewarm_overlays<R: Runtime>(app: &AppHandle<R>) {
    match capture::monitor_geometry() {
        Ok(monitors) => {
            if let Err(e) = ensure_overlays(app, &monitors) {
                eprintln!("[overlay] {e}");
            }
        }
        Err(e) => eprintln!("[overlay] {e}"),
    }
}

/// Tell every overlay to fetch and paint its bitmap.
pub fn start_overlays<R: Runtime>(app: &AppHandle<R>, monitors: &[MonitorInfo]) {
    for info in monitors {
        let label = overlay_label(info.geom.id);
        if let Err(e) = app.emit_to(EventTarget::webview_window(&label), "capture:start", info) {
            eprintln!("[overlay] cannot signal {label}: {e}");
        }
    }
}

pub fn show_overlay<R: Runtime>(app: &AppHandle<R>, monitor_id: u32) {
    if let Some(window) = app.get_webview_window(&overlay_label(monitor_id)) {
        let _ = window.show();
    }
}

/// Make the overlay the key window so it receives keyboard and mouse-move
/// events. Returns false while the window is not visible yet (retry later).
pub fn focus_overlay<R: Runtime>(app: &AppHandle<R>, monitor_id: u32) -> bool {
    let Some(window) = app.get_webview_window(&overlay_label(monitor_id)) else {
        return false;
    };
    if !window.is_visible().unwrap_or(false) {
        return false;
    }
    #[cfg(target_os = "macos")]
    crate::macos::activate_app(app);
    let _ = window.set_focus();
    true
}

pub fn overlay_is_focused<R: Runtime>(app: &AppHandle<R>, monitor_id: u32) -> bool {
    app.get_webview_window(&overlay_label(monitor_id))
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

/// One overlay owns the selection now: tell the others to stay dimmed and
/// ignore input.
pub fn lock_overlays<R: Runtime>(app: &AppHandle<R>, except_monitor_id: u32) {
    let keep = overlay_label(except_monitor_id);
    for (label, _) in app.webview_windows() {
        if label.starts_with(OVERLAY_PREFIX) && label != keep {
            let _ = app.emit_to(EventTarget::webview_window(&label), "capture:lock", ());
        }
    }
}

/// Hide (not destroy) the overlays and let them drop their bitmaps.
pub fn hide_overlays<R: Runtime>(app: &AppHandle<R>) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(OVERLAY_PREFIX) {
            let _ = app.emit_to(EventTarget::webview_window(&label), "capture:reset", ());
            let _ = window.hide();
        }
    }
}

pub fn open_editor<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(EDITOR) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return window
            .emit("editor:reload", ())
            .map_err(|e| e.to_string());
    }
    debug::log("opening editor window");
    WebviewWindowBuilder::new(app, EDITOR, WebviewUrl::App("index.html".into()))
        .title("Socorin")
        .inner_size(1100.0, 740.0)
        .min_inner_size(640.0, 420.0)
        .center()
        .visible(true)
        .build()
        .map(|_| ())
        .map_err(|e| format!("cannot create editor window: {e}"))
}

pub fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// First-launch dialog (logo, "lives in the menu bar", OK). Created hidden;
/// the page shows it once it has rendered (`welcome_ready`), so there is no
/// blank flash. No close button: OK is the only way out, and it records that
/// the dialog must not come back.
pub fn open_welcome<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if app.get_webview_window(WELCOME).is_some() {
        show_welcome(app);
        return Ok(());
    }
    WebviewWindowBuilder::new(app, WELCOME, WebviewUrl::App("index.html".into()))
        .title("Welcome to Socorin")
        .inner_size(440.0, 430.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .closable(false)
        .center()
        .visible(false)
        .build()
        .map(|_| ())
        .map_err(|e| format!("cannot create welcome window: {e}"))
}

pub fn show_welcome<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(WELCOME) {
        // A menu-bar app is not active, so a plain show() would leave the
        // dialog behind other apps' windows.
        #[cfg(target_os = "macos")]
        crate::macos::activate_app(app);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn close_welcome<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(WELCOME) {
        let _ = window.close();
    }
}

/// Transparent room around the recording bar for its drop shadow. The page
/// draws the bar this far from the window's edges (`RECORDER_MARGIN` in
/// `src/lib/ipc.ts` must match).
pub const RECORDER_MARGIN: f64 = 20.0;
/// Size the hidden window is created with (the 640 px bar plus its margin);
/// it is resized to the measured bar on use.
const RECORDER_DEFAULT_SIZE: (f64, f64) = (680.0, 84.0);

/// The floating recording bar (elapsed time, Stop & copy, Stop, Cancel).
/// It is the same toolbar the overlay shows before recording, in its own
/// transparent always-on-top window placed exactly where that one was, so
/// the buttons do not move when the recording starts. Created hidden once
/// and reused.
fn create_recorder<R: Runtime>(app: &AppHandle<R>, x: f64, y: f64, w: f64, h: f64) -> Result<tauri::WebviewWindow<R>, String> {
    let window = WebviewWindowBuilder::new(app, RECORDER, WebviewUrl::App("index.html".into()))
        .title("Recording")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .accept_first_mouse(true)
        .focused(false)
        .visible(false)
        .position(x, y)
        .inner_size(w, h)
        .build()
        .map_err(|e| format!("cannot create the recording bar: {e}"))?;
    #[cfg(all(target_os = "macos", not(test)))]
    crate::macos::raise_window(&window, crate::macos::RECORDER_LEVEL);
    Ok(window)
}

/// Create the recording bar's window ahead of time, hidden, so it appears
/// without a blank flash the first time a recording starts.
pub fn prewarm_recorder<R: Runtime>(app: &AppHandle<R>) {
    if app.get_webview_window(RECORDER).is_some() {
        return;
    }
    let (w, h) = RECORDER_DEFAULT_SIZE;
    if let Err(e) = create_recorder(app, 0.0, 0.0, w, h) {
        eprintln!("[record] {e}");
    }
}

/// The recorder window's frame (global logical x, y, width, height) for a
/// bar at `bar` on `monitor`: the bar plus its transparent margin.
pub fn recorder_frame(monitor: &MonitorGeom, bar: &record::Bar) -> (f64, f64, f64, f64) {
    let m = RECORDER_MARGIN;
    (
        monitor.x as f64 + bar.x - m,
        monitor.y as f64 + bar.y - m,
        bar.width + 2.0 * m,
        bar.height + 2.0 * m,
    )
}

/// Show the recording bar where the overlay's Record bar is (`bar` is
/// relative to the monitor, in logical pixels).
pub fn show_recorder<R: Runtime>(app: &AppHandle<R>, monitor: &MonitorGeom, bar: &record::Bar) {
    let (x, y, w, h) = recorder_frame(monitor, bar);
    let window = match app.get_webview_window(RECORDER) {
        Some(window) => window,
        None => match create_recorder(app, x, y, w, h) {
            Ok(window) => window,
            Err(e) => {
                eprintln!("[record] {e}");
                return;
            }
        },
    };
    let _ = window.set_position(LogicalPosition::new(x, y));
    let _ = window.set_size(LogicalSize::new(w, h));
    let _ = window.show();
}

pub fn hide_recorder<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(RECORDER) {
        let _ = window.hide();
    }
}

/// Hide the bar after `delay` (it shows "Copied" for a moment), unless a
/// new recording has claimed it meanwhile.
pub fn hide_recorder_later<R: Runtime>(app: &AppHandle<R>, delay: std::time::Duration) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if !record::is_recording(&app) {
            hide_recorder(&app);
        }
    });
}

/// Logical size of the update popover's window: the card plus a
/// transparent margin all round for its shadow (`UPDATE_MARGIN`, which
/// `.update-notice` in `src/styles.css` must match).
pub const UPDATE_SIZE: (f64, f64) = (392.0, 170.0);
pub const UPDATE_MARGIN: f64 = 16.0;

/// The small "new version" popover under the menu bar / tray icon. Created
/// hidden once and reused; the page calls `update_ready` when it has
/// rendered, which shows it (`reveal_update_notice`) without a blank flash.
/// It is never focused when shown, so the user's work keeps the keyboard;
/// once clicked, a click elsewhere hides it again (`update::blurred`).
pub fn show_update_notice<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    show_popover(app, UPDATE, "Socorin update", UPDATE_SIZE, update_notice_position(app))
}

/// The "Link copied" popover after an upload (`share::announce`): the same
/// window as the update notice, under the same icon, with its own label so
/// the two never fight over one window.
///
/// `anchor`: the button that was clicked (or the selection), in screen
/// logical pixels; the popover goes right under it (above it in the lower
/// half of the screen), like the update notice goes under the icon.
pub fn show_share_notice<R: Runtime>(app: &AppHandle<R>, anchor: Option<share::Anchor>) -> Result<(), String> {
    let position = match anchor {
        Some(a) => place((a.x, a.y, a.width, a.height), work_area_around(app, &a), SHARE_SIZE),
        None => update_notice_position(app),
    };
    #[cfg(test)]
    {
        *app.state::<share::ShareState>().placed.lock().unwrap() = Some(position);
    }
    show_popover(app, SHARE, "Socorin share", SHARE_SIZE, position)
}

/// A page's anchor (CSS pixels of `window`) as screen logical pixels.
/// `None` for values that make no sense: a page is not trusted to put a
/// window anywhere it likes.
pub fn anchor_on_screen<R: Runtime>(window: &tauri::Window<R>, a: share::Anchor) -> Option<share::Anchor> {
    let sane = |v: f64| v.is_finite() && v.abs() < 100_000.0;
    if !(sane(a.x) && sane(a.y) && sane(a.width) && sane(a.height)) || a.width < 0.0 || a.height < 0.0 {
        return None;
    }
    let scale = window.scale_factor().ok()?.max(0.1);
    let origin = window.inner_position().ok()?;
    Some(share::Anchor {
        x: origin.x as f64 / scale + a.x,
        y: origin.y as f64 / scale + a.y,
        width: a.width,
        height: a.height,
    })
}

/// The work area of the monitor the anchor's centre is on (logical), the
/// primary monitor's failing that.
fn work_area_around<R: Runtime>(app: &AppHandle<R>, a: &share::Anchor) -> (f64, f64, f64, f64) {
    // The mock runtime has no monitors (its monitor calls panic).
    #[cfg(test)]
    {
        let _ = (app, a);
        TEST_WORK_AREA
    }
    #[cfg(not(test))]
    real_work_area_around(app, a)
}

#[cfg(test)]
pub const TEST_WORK_AREA: (f64, f64, f64, f64) = (0.0, 0.0, 1440.0, 900.0);

#[cfg(not(test))]
fn real_work_area_around<R: Runtime>(app: &AppHandle<R>, a: &share::Anchor) -> (f64, f64, f64, f64) {
    let (cx, cy) = (a.x + a.width / 2.0, a.y + a.height / 2.0);
    let on = app.available_monitors().unwrap_or_default().into_iter().find(|m| {
        let s = m.scale_factor().max(0.1);
        let (mx, my) = (m.position().x as f64 / s, m.position().y as f64 / s);
        let (mw, mh) = (m.size().width as f64 / s, m.size().height as f64 / s);
        cx >= mx && cx < mx + mw && cy >= my && cy < my + mh
    });
    match on.or_else(|| app.primary_monitor().ok().flatten()) {
        Some(monitor) => logical_work_area(&monitor),
        None => (0.0, 0.0, f64::MAX / 4.0, f64::MAX / 4.0),
    }
}

pub fn reveal_share_notice<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(SHARE) {
        let _ = window.show();
    }
}

pub fn hide_share_notice<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(SHARE) {
        let _ = window.hide();
    }
}

/// Logical size of the share popover (wider and taller than the update
/// one: the whole link on one line, the retention warning, three buttons).
pub const SHARE_SIZE: (f64, f64) = (480.0, 216.0);

/// A popover window under the menu bar / tray icon, created hidden once and
/// reused (see `show_update_notice` for how it is revealed).
fn show_popover<R: Runtime>(app: &AppHandle<R>, label: &str, title: &str, size: (f64, f64), position: (f64, f64)) -> Result<(), String> {
    let (x, y) = position;
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.set_position(LogicalPosition::new(x, y));
        let _ = window.show();
        return Ok(());
    }
    let (w, h) = size;
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .accept_first_mouse(true)
        .focused(false)
        .visible(false)
        .position(x, y)
        .inner_size(w, h)
        .build()
        .map(|_| ())
        .map_err(|e| format!("cannot create the {label} popover: {e}"))
}

pub fn reveal_update_notice<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(UPDATE) {
        let _ = window.show();
    }
}

pub fn hide_update_notice<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(UPDATE) {
        let _ = window.hide();
    }
}

/// Top-left corner (logical) for the popover: under the tray icon when the
/// system says where it is (macOS, Windows), else the top right of the
/// primary monitor (Linux tray icons have no position), else a corner.
fn update_notice_position<R: Runtime>(app: &AppHandle<R>) -> (f64, f64) {
    // The mock runtime has no monitors (its monitor calls panic).
    #[cfg(test)]
    {
        let _ = app;
        return (UPDATE_MARGIN, UPDATE_MARGIN);
    }
    #[cfg(not(test))]
    real_update_notice_position(app)
}

#[cfg(not(test))]
fn real_update_notice_position<R: Runtime>(app: &AppHandle<R>) -> (f64, f64) {
    let monitors = app.available_monitors().unwrap_or_default();
    let icon = app
        .tray_by_id(crate::tray::TRAY_ID)
        .and_then(|tray| tray.rect().ok().flatten());
    if let Some((icon, area)) = icon.and_then(|rect| icon_and_work_area(&rect, &monitors)) {
        return place(icon, area, UPDATE_SIZE);
    }
    match app.primary_monitor().ok().flatten() {
        Some(monitor) => {
            let (ax, ay, aw, _) = logical_work_area(&monitor);
            (ax + aw - UPDATE_SIZE.0, ay)
        }
        None => (UPDATE_MARGIN, UPDATE_MARGIN),
    }
}

/// The tray icon's rectangle and the work area of the monitor it is on, in
/// that monitor's logical pixels. The system reports the icon in physical
/// pixels (a logical one is taken as is).
fn icon_and_work_area(rect: &Rect, monitors: &[Monitor]) -> Option<((f64, f64, f64, f64), (f64, f64, f64, f64))> {
    let (px, py) = match rect.position {
        Position::Physical(p) => (p.x as f64, p.y as f64),
        Position::Logical(l) => (l.x, l.y),
    };
    let (pw, ph) = match rect.size {
        Size::Physical(s) => (s.width as f64, s.height as f64),
        Size::Logical(s) => (s.width, s.height),
    };
    let monitor = monitors.iter().find(|m| {
        let (mx, my) = (m.position().x as f64, m.position().y as f64);
        let (mw, mh) = (m.size().width as f64, m.size().height as f64);
        px >= mx && px < mx + mw && py >= my && py < my + mh
    })?;
    let s = monitor.scale_factor().max(0.1);
    Some(((px / s, py / s, pw / s, ph / s), logical_work_area(monitor)))
}

fn logical_work_area(monitor: &Monitor) -> (f64, f64, f64, f64) {
    let s = monitor.scale_factor().max(0.1);
    let area = monitor.work_area();
    (
        area.position.x as f64 / s,
        area.position.y as f64 / s,
        area.size.width as f64 / s,
        area.size.height as f64 / s,
    )
}

/// Where a popover of `size` goes for a tray icon at `icon` (x, y, w, h)
/// on the work area `area` (x, y, w, h), all logical: centred under the
/// icon, or above it when the icon sits in the lower half (a Windows
/// taskbar at the bottom), and always inside the work area. The window's
/// transparent margin provides the visual gap.
pub fn place(icon: (f64, f64, f64, f64), area: (f64, f64, f64, f64), size: (f64, f64)) -> (f64, f64) {
    let (ix, iy, iw, ih) = icon;
    let (ax, ay, aw, ah) = area;
    let (w, h) = size;
    let below = iy + ih / 2.0 < ay + ah / 2.0;
    let x = (ix + iw / 2.0 - w / 2.0).clamp(ax, (ax + aw - w).max(ax));
    let y = if below { iy + ih } else { iy - h };
    (x, y.clamp(ay, (ay + ah - h).max(ay)))
}

#[cfg(target_os = "linux")]
pub fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capture::{Mode, MonitorInfo},
        record::Bar,
        test_support::{app, geom},
    };
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    #[test]
    fn overlay_labels_carry_the_monitor_id() {
        assert_eq!(overlay_label(3), "overlay-3");
        assert!(overlay_label(12).starts_with(OVERLAY_PREFIX));
    }

    #[test]
    fn ensure_overlays_creates_one_hidden_window_per_monitor_and_reuses_it() {
        let app = app();
        let monitors = [geom(101, 0, 0, 800, 600, 2.0, true), geom(102, 800, 0, 1024, 768, 1.0, false)];
        ensure_overlays(&app, &monitors).unwrap();
        assert!(app.get_webview_window("overlay-101").is_some());
        assert!(app.get_webview_window("overlay-102").is_some());
        let before = app.webview_windows().len();

        // Same layout again: the windows are kept.
        ensure_overlays(&app, &monitors).unwrap();
        assert_eq!(app.webview_windows().len(), before);

        // A monitor went away: its overlay is dropped (best effort).
        ensure_overlays(&app, &monitors[..1]).unwrap();
        assert!(app.get_webview_window("overlay-101").is_some());
    }

    #[test]
    fn overlay_signals_go_to_the_matching_windows() {
        let app = app();
        let geoms = [geom(111, 0, 0, 100, 100, 1.0, true), geom(112, 100, 0, 100, 100, 1.0, false)];
        ensure_overlays(&app, &geoms).unwrap();
        let infos: Vec<MonitorInfo> = geoms
            .iter()
            .map(|g| MonitorInfo {
                geom: g.clone(),
                image_width: 100,
                image_height: 100,
                session: 1,
                preselect_full: false,
                mode: Mode::Screenshot,
            })
            .collect();
        start_overlays(&app, &infos);
        show_overlay(&app, 111);
        show_overlay(&app, 999);
        assert!(!overlay_is_focused(&app, 111));
        assert!(!overlay_is_focused(&app, 999));
        assert!(!focus_overlay(&app, 999));
        lock_overlays(&app, 111);
        hide_overlays(&app);
    }

    #[test]
    fn prewarm_creates_overlays_for_the_real_monitors() {
        let app = app();
        prewarm_overlays(&app);
        let geoms = crate::capture::monitor_geometry().unwrap();
        for g in geoms {
            assert!(app.get_webview_window(&overlay_label(g.id)).is_some());
        }
    }

    #[test]
    fn editor_settings_and_welcome_windows() {
        let app = app();
        open_editor(&app).unwrap();
        assert!(app.get_webview_window(EDITOR).is_some());
        open_editor(&app).unwrap(); // reused

        show_settings(&app); // no main window in the tests: nothing happens
        WebviewWindowBuilder::new(&*app, MAIN, WebviewUrl::App("index.html".into()))
            .visible(false)
            .build()
            .ok();
        show_settings(&app);

        close_welcome(&app);
        if app.get_webview_window(WELCOME).is_none() {
            open_welcome(&app).unwrap();
        }
        assert!(app.get_webview_window(WELCOME).is_some());
        close_welcome(&app);
    }

    #[test]
    fn the_recording_bar_takes_the_place_of_the_overlay_bar() {
        let monitor = geom(1, 100, 50, 1000, 800, 1.0, true);
        let bar = Bar { x: 300.0, y: 500.0, width: 400.0, height: 42.0 };
        // 20 px of transparent margin around the bar, from the monitor's origin.
        assert_eq!(recorder_frame(&monitor, &bar), (100.0 + 300.0 - 20.0, 50.0 + 500.0 - 20.0, 440.0, 82.0));
        assert_eq!(RECORDER_MARGIN, 20.0);

        // The mock runtime keeps no geometry or visibility, so the window
        // calls are exercised for their side-effect-free happy paths only.
        let app = app();
        prewarm_recorder(&app);
        prewarm_recorder(&app); // already there
        assert!(app.get_webview_window(RECORDER).is_some());
        show_recorder(&app, &monitor, &bar);
        hide_recorder(&app);
        hide_recorder_later(&app, std::time::Duration::from_millis(5));
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(app.get_webview_window(RECORDER).is_some());
    }

    #[test]
    fn the_update_popover_sits_under_the_icon_inside_the_work_area() {
        let size = (392.0, 170.0);
        // macOS: icon in the menu bar at the top; centred below it.
        assert_eq!(place((1200.0, 0.0, 30.0, 24.0), (0.0, 24.0, 1440.0, 876.0), size), (1019.0, 24.0));
        // Near the right edge: pushed left to stay on screen.
        assert_eq!(place((1420.0, 0.0, 20.0, 24.0), (0.0, 24.0, 1440.0, 876.0), size), (1048.0, 24.0));
        // Windows: taskbar at the bottom, icon in its lower half: above it,
        // and pushed left so the 392 px card ends at the screen's edge.
        assert_eq!(place((1800.0, 1040.0, 24.0, 24.0), (0.0, 0.0, 1920.0, 1040.0), size), (1528.0, 870.0));
        assert_eq!(place((900.0, 1040.0, 24.0, 24.0), (0.0, 0.0, 1920.0, 1040.0), size), (716.0, 870.0));
        // A work area smaller than the popover: pinned to its origin.
        assert_eq!(place((10.0, 0.0, 10.0, 10.0), (0.0, 0.0, 100.0, 100.0), size), (0.0, 0.0));

        // The mock runtime knows no monitors: the corner fallback, and the
        // window is created hidden, then shown, hidden again and reused.
        let app = app();
        assert_eq!(update_notice_position(&app), (UPDATE_MARGIN, UPDATE_MARGIN));
        assert!(icon_and_work_area(
            &Rect { position: Position::Physical(tauri::PhysicalPosition::new(5, 5)), size: Size::Physical(tauri::PhysicalSize::new(20, 20)) },
            &[]
        )
        .is_none());
        assert!(icon_and_work_area(
            &Rect { position: Position::Logical(tauri::LogicalPosition::new(5.0, 5.0)), size: Size::Logical(tauri::LogicalSize::new(20.0, 20.0)) },
            &[]
        )
        .is_none());
        reveal_update_notice(&app); // nothing yet
        hide_update_notice(&app);
        show_update_notice(&app).unwrap();
        assert!(app.get_webview_window(UPDATE).is_some());
        reveal_update_notice(&app);
        hide_update_notice(&app);
        show_update_notice(&app).unwrap(); // reused
        assert_eq!(app.webview_windows().values().filter(|w| w.label() == UPDATE).count(), 1);
        assert_eq!(UPDATE_MARGIN, 16.0);

        // The share popover is its own window of the same kind.
        reveal_share_notice(&app);
        hide_share_notice(&app);
        show_share_notice(&app, None).unwrap();
        assert!(app.get_webview_window(SHARE).is_some());
        reveal_share_notice(&app);
        hide_share_notice(&app);
        show_share_notice(&app, None).unwrap(); // reused
        assert_eq!(app.webview_windows().values().filter(|w| w.label() == SHARE).count(), 1);
        assert!(SHARE_SIZE.1 > UPDATE_SIZE.1);
    }

    #[test]
    fn the_share_popover_sits_under_the_anchor_it_was_given() {
        use crate::share::{Anchor, ShareState};
        let app = app();
        let state = app.state::<ShareState>();
        let (w, h) = SHARE_SIZE;
        // Under a button in the upper half, centred; above one in the lower half.
        show_share_notice(&app, Some(Anchor { x: 700.0, y: 200.0, width: 100.0, height: 30.0 })).unwrap();
        assert_eq!(*state.placed.lock().unwrap(), Some((750.0 - w / 2.0, 230.0)));
        show_share_notice(&app, Some(Anchor { x: 700.0, y: 800.0, width: 100.0, height: 30.0 })).unwrap();
        assert_eq!(*state.placed.lock().unwrap(), Some((750.0 - w / 2.0, 800.0 - h)));
        // Never outside the work area.
        show_share_notice(&app, Some(Anchor { x: 1430.0, y: 10.0, width: 10.0, height: 10.0 })).unwrap();
        assert_eq!(*state.placed.lock().unwrap(), Some((1440.0 - w, 20.0)));
        // Without one: under the icon, like the update notice.
        show_share_notice(&app, None).unwrap();
        assert_eq!(*state.placed.lock().unwrap(), Some((UPDATE_MARGIN, UPDATE_MARGIN)));

        // A page's anchor is taken relative to its window (the mock runtime
        // puts every window at the origin, scale 1), and only when it makes
        // sense.
        let window = app.get_webview_window(SHARE).unwrap();
        let window = window.as_ref().window();
        let a = Anchor { x: 10.0, y: 20.0, width: 30.0, height: 40.0 };
        assert_eq!(anchor_on_screen(&window, a), Some(a));
        for bad in [
            Anchor { x: f64::NAN, ..a },
            Anchor { y: f64::INFINITY, ..a },
            Anchor { width: -1.0, ..a },
            Anchor { x: 1e9, ..a },
        ] {
            assert_eq!(anchor_on_screen(&window, bad), None, "{bad:?}");
        }
        hide_share_notice(&app);
    }
}
