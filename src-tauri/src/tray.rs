//! System tray / menu bar icon.
//!
//! Idle, the menu offers the captures and recordings. While a recording
//! runs the icon is a steady red dot (no blinking: it distracts), the
//! elapsed time sits next to it (macOS) and the menu becomes the
//! recording's remote control: the running time, Stop & Copy, Stop &
//! Upload Link, Stop, Cancel. That is the whole UI of a full-screen
//! recording, which shows no floating bar.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Manager, Runtime,
};

use crate::{capture, record, settings, update, windows};

pub const TRAY_ID: &str = "main";

/// How often the running time next to the icon is refreshed.
const TICK: Duration = Duration::from_millis(500);

/// How far up (in pixels of the 36 px icon) the recording dot is drawn
/// from the image centre. The macOS status bar centres the 18 pt image on
/// the button but sets the title on the menu-bar text baseline, whose
/// digits sit about 0.75 pt above that centre (measured: digits at
/// 14.25 pt, the other apps' icons at 15 pt in a 30 pt bar); lifting the
/// dot the same amount puts it on one line with the running time. The
/// idle logo has no title next to it and stays centred like the
/// neighbouring icons.
#[cfg(target_os = "macos")]
const DOT_LIFT: f32 = 1.5;
#[cfg(not(target_os = "macos"))]
const DOT_LIFT: f32 = 0.0;

/// What the tray swaps between while the app runs.
pub struct TrayItems<R: Runtime> {
    idle_menu: Menu<R>,
    recording_menu: Menu<R>,
    /// "Recording 00:12", refreshed every second.
    timer: MenuItem<R>,
    /// "Update to Socorin 1.2.3…" and its separator, at the top of the idle
    /// menu while a newer version is known (`set_update_available`).
    update_item: MenuItem<R>,
    update_separator: PredefinedMenuItem<R>,
    update_shown: AtomicBool,
    /// Bumped on every state change; a clock thread quits when its number
    /// is stale. `lock` serialises the icon updates with the state changes
    /// so a late tick can never leave a red icon behind.
    generation: AtomicU64,
    lock: Mutex<()>,
}

impl<R: Runtime> TrayItems<R> {
    /// Put the "Update to …" line at the top of the idle menu, or take it
    /// out again.
    fn set_update_available(&self, version: Option<&str>) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match version {
            Some(v) => {
                let _ = self.update_item.set_text(update_text(v));
                if !self.update_shown.swap(true, Ordering::SeqCst) {
                    let _ = self.idle_menu.insert(&self.update_separator, 0);
                    let _ = self.idle_menu.insert(&self.update_item, 0);
                }
            }
            None => {
                if self.update_shown.swap(false, Ordering::SeqCst) {
                    let _ = self.idle_menu.remove(&self.update_item);
                    let _ = self.idle_menu.remove(&self.update_separator);
                }
            }
        }
    }
}

/// The menu line offering a newer version.
pub fn update_text(version: &str) -> String {
    format!("Update to Socorin {version}…")
}

/// The tooltip of the idle icon.
fn idle_tooltip(update_available: Option<&str>) -> String {
    match update_available {
        Some(v) => format!("Socorin – version {v} available"),
        None => "Socorin".into(),
    }
}

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let items = build_items(app)?;
    let idle_menu = items.idle_menu.clone();
    app.manage(items);

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&idle_menu)
        .show_menu_on_left_click(true)
        .tooltip(idle_tooltip(None))
        .on_menu_event(|app, event| match event.id().as_ref() {
            "capture" => capture::begin_region(app.clone()),
            "capture_full" => capture::begin_fullscreen(app.clone()),
            "record" => record::begin_region(app.clone()),
            "record_full" => record::begin_fullscreen(app.clone()),
            "stop_copy" => record::stop_async_with(app.clone(), record::Outcome::Copy),
            "stop_upload" => record::stop_async_with(app.clone(), record::Outcome::Upload),
            "stop_record" => record::stop_async_with(app.clone(), record::Outcome::Reveal),
            "cancel_record" => record::stop_async_with(app.clone(), record::Outcome::Discard),
            "update" => update::install(app),
            "settings" => windows::show_settings(app),
            "quit" => {
                record::stop_if_recording(app);
                app.exit(0);
            }
            _ => {}
        });

    let builder = match idle_icon(app) {
        Some(icon) => builder.icon(icon).icon_as_template(cfg!(target_os = "macos")),
        None => builder,
    };

    builder.build(app)?;
    Ok(())
}

/// The menus and their swappable lines (everything but the icon itself).
fn build_items<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayItems<R>> {
    let update_item = MenuItem::with_id(app, "update", update_text("?"), true, None::<&str>)?;
    let update_separator = PredefinedMenuItem::separator(app)?;
    let capture_item = MenuItem::with_id(app, "capture", "Capture Region", true, None::<&str>)?;
    let full_item = MenuItem::with_id(app, "capture_full", "Capture Full Screen", true, None::<&str>)?;
    let record_item = MenuItem::with_id(app, "record", "Record Region", true, None::<&str>)?;
    let record_full_item =
        MenuItem::with_id(app, "record_full", "Record Full Screen", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let idle_menu = Menu::with_items(
        app,
        &[
            &capture_item,
            &full_item,
            &PredefinedMenuItem::separator(app)?,
            &record_item,
            &record_full_item,
            &PredefinedMenuItem::separator(app)?,
            &settings_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let timer = MenuItem::with_id(app, "timer", &timer_text(0), false, None::<&str>)?;
    // `&&`: a literal ampersand (a single `&` marks a mnemonic).
    let stop_copy_item =
        MenuItem::with_id(app, "stop_copy", "Stop && Copy to Clipboard", true, None::<&str>)?;
    let stop_upload_item =
        MenuItem::with_id(app, "stop_upload", "Stop && Upload Link", true, None::<&str>)?;
    let stop_item = MenuItem::with_id(app, "stop_record", "Stop", true, None::<&str>)?;
    let cancel_item = MenuItem::with_id(app, "cancel_record", "Cancel Recording", true, None::<&str>)?;
    let recording_menu = Menu::with_items(
        app,
        &[
            &timer,
            &PredefinedMenuItem::separator(app)?,
            &stop_copy_item,
            &stop_upload_item,
            &stop_item,
            &cancel_item,
            &PredefinedMenuItem::separator(app)?,
            &settings_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;
    Ok(TrayItems {
        idle_menu,
        recording_menu,
        timer,
        update_item,
        update_separator,
        update_shown: AtomicBool::new(false),
        generation: AtomicU64::new(0),
        lock: Mutex::new(()),
    })
}

/// Offer (or stop offering) a newer version at the top of the tray menu
/// and in the tooltip.
pub fn set_update_available<R: Runtime>(app: &AppHandle<R>, version: Option<&str>) {
    let Some(items) = app.try_state::<TrayItems<R>>() else {
        return;
    };
    items.set_update_available(version);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if !record::is_recording(app) {
            let _ = tray.set_tooltip(Some(idle_tooltip(version)));
        }
    }
}

/// Reflect a running recording: the icon turns red, the menu becomes the
/// recording's controls and a thread keeps the elapsed time current until
/// the recording ends.
pub fn set_recording<R: Runtime>(app: &AppHandle<R>, on: bool) {
    let Some(items) = app.try_state::<TrayItems<R>>() else {
        return;
    };
    let _guard = items.lock.lock().unwrap_or_else(|e| e.into_inner());
    let generation = items.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if on {
        let _ = items.timer.set_text(timer_text(0));
        let _ = tray.set_menu(Some(items.recording_menu.clone()));
        show_recording(&tray, 0);
        let app = app.clone();
        std::thread::spawn(move || tick(app, generation));
    } else {
        let _ = tray.set_menu(Some(items.idle_menu.clone()));
        show_idle(app, &tray);
    }
}

/// Refresh the running time every `TICK` while this generation records.
fn tick<R: Runtime>(app: AppHandle<R>, generation: u64) {
    loop {
        std::thread::sleep(TICK);
        let items = app.state::<TrayItems<R>>();
        let _guard = items.lock.lock().unwrap_or_else(|e| e.into_inner());
        if items.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let status = record::status(&app);
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return;
        };
        if !status.recording {
            return;
        }
        let elapsed = now_ms().saturating_sub(status.started_ms);
        let _ = items.timer.set_text(timer_text(elapsed));
        show_recording(&tray, elapsed);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The red dot plus the elapsed time in the menu bar and the tooltip.
fn show_recording<R: Runtime>(tray: &TrayIcon<R>, elapsed_ms: u64) {
    let _ = tray.set_icon(Some(recording_icon()));
    let _ = tray.set_icon_as_template(false);
    let clock = format_elapsed(elapsed_ms);
    let _ = tray.set_title(Some(&clock));
    let _ = tray.set_tooltip(Some(format!("Socorin – recording {clock}")));
}

fn show_idle<R: Runtime>(app: &AppHandle<R>, tray: &TrayIcon<R>) {
    if let Some(icon) = idle_icon(app) {
        let _ = tray.set_icon(Some(icon));
        let _ = tray.set_icon_as_template(cfg!(target_os = "macos"));
    }
    // `None` leaves the old text in place on macOS; an empty title clears it.
    let _ = tray.set_title(Some(""));
    let settings = settings::current(app);
    let available = (!settings.update_available.is_empty()).then_some(settings.update_available.as_str());
    let _ = tray.set_tooltip(Some(idle_tooltip(available)));
}

/// The menu line that shows how long the recording has been running.
pub fn timer_text(elapsed_ms: u64) -> String {
    format!("Recording {}", format_elapsed(elapsed_ms))
}

/// `mm:ss`, or `h:mm:ss` from the first hour on (same as the bar).
pub fn format_elapsed(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

/// The icon shown while idle: the logo as a monochrome template image on
/// macOS (it follows the menu bar's light / dark look), the app icon
/// elsewhere.
fn idle_icon<R: Runtime>(app: &AppHandle<R>) -> Option<Image<'static>> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        Some(template_icon())
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.default_window_icon().cloned().map(Image::to_owned)
    }
}

/// A red disc (36x36 px, drawn at 18pt) shown in the menu bar / tray while
/// recording. Not a template image, so it keeps its colour.
fn recording_icon() -> Image<'static> {
    const SIZE: u32 = 36;
    const SS: u32 = 4;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32 - 18.0;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32 - 18.0 + DOT_LIFT;
                    if x * x + y * y <= 9.0 * 9.0 {
                        hits += 1;
                    }
                }
            }
            let alpha = (hits * 255 / (SS * SS)) as u8;
            rgba.extend_from_slice(&[255, 59, 48, alpha]);
        }
    }
    Image::new_owned(rgba, SIZE, SIZE)
}

/// The app logo as a menu bar glyph (36x36 px, rendered at 18pt): the
/// squircle of `design/logo.svg`, filled, with the S cut out of it. A
/// template image, so macOS paints it in the menu bar's own colour. Drawn
/// procedurally from the same geometry as the SVG (a superellipse and the
/// S's cubic Bézier spine), 4x4 supersampled so it stays crisp on Retina,
/// and computed once.
#[cfg(target_os = "macos")]
fn template_icon() -> Image<'static> {
    use std::sync::OnceLock;
    static ICON: OnceLock<Vec<u8>> = OnceLock::new();
    let rgba = ICON.get_or_init(render_logo_glyph);
    Image::new_owned(rgba.clone(), LOGO_ICON_SIZE, LOGO_ICON_SIZE)
}

/// Icon canvas (px) and how much of it the logo takes (16 of 18 pt, the
/// size of the other menu bar icons, ChatGPT's for one).
#[cfg(target_os = "macos")]
const LOGO_ICON_SIZE: u32 = 36;
#[cfg(target_os = "macos")]
const LOGO_GLYPH_PX: f32 = 32.0;

/// The spine of the S in `design/logo.svg`, in the logo's 1024-unit space:
/// the `M … C …` path of the SVG, six cubic Bézier segments from the top
/// right end over the upper bowl, through the diagonal waist and round the
/// wider lower bowl to the bottom left end. Kept in step with the SVG by
/// hand; the test below checks the ends.
#[cfg(target_os = "macos")]
const LOGO_S_PATH: [[(f32, f32); 4]; 6] = [
    [(668.0, 354.0), (652.0, 291.0), (589.0, 267.0), (518.0, 267.0)],
    [(518.0, 267.0), (427.0, 267.0), (356.0, 313.0), (356.0, 381.0)],
    [(356.0, 381.0), (356.0, 449.0), (420.0, 477.0), (512.0, 505.0)],
    [(512.0, 505.0), (604.0, 533.0), (684.0, 565.0), (684.0, 641.0)],
    [(684.0, 641.0), (684.0, 712.0), (611.0, 757.0), (516.0, 757.0)],
    [(516.0, 757.0), (435.0, 757.0), (365.0, 726.0), (348.0, 655.0)],
];

/// The S as a dense polyline (about 3 units between samples, well under
/// the 54-unit stroke radius the rasteriser tests against).
#[cfg(target_os = "macos")]
fn logo_s_points() -> Vec<(f32, f32)> {
    const STEPS: u32 = 100;
    let mut pts = Vec::with_capacity(LOGO_S_PATH.len() * STEPS as usize + 1);
    for [p0, p1, p2, p3] in LOGO_S_PATH {
        for i in 0..=STEPS {
            let t = i as f32 / STEPS as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            pts.push((a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1));
        }
    }
    pts
}

#[cfg(target_os = "macos")]
fn render_logo_glyph() -> Vec<u8> {
    const SS: u32 = 4;
    const STROKE_HALF: f32 = 54.0; // the S is a 108-unit stroke
    const SQUIRCLE_R: f32 = 504.0; // the squircle spans 8..1016
    let scale = LOGO_GLYPH_PX / 1024.0;
    let ox = (LOGO_ICON_SIZE as f32 - LOGO_GLYPH_PX) / 2.0;
    let oy = ox;
    let s_points = logo_s_points();
    let on_s = |lx: f32, ly: f32| {
        s_points
            .iter()
            .any(|(px, py)| (lx - px).powi(2) + (ly - py).powi(2) <= STROKE_HALF * STROKE_HALF)
    };
    let inside = |x: f32, y: f32| {
        let lx = (x - ox) / scale;
        let ly = (y - oy) / scale;
        let (dx, dy) = ((lx - 512.0).abs() / SQUIRCLE_R, (ly - 512.0).abs() / SQUIRCLE_R);
        dx.powi(4) + dy.powi(4) <= 1.0 && !on_s(lx, ly)
    };

    let size = LOGO_ICON_SIZE;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if inside(x, y) {
                        hits += 1;
                    }
                }
            }
            let alpha = (hits * 255 / (SS * SS)) as u8;
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::app;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_menu_bar_glyph_is_the_logo_squircle_with_the_s_cut_out() {
        let icon = template_icon();
        assert_eq!((icon.width(), icon.height()), (36, 36));
        let rgba = icon.rgba();
        assert_eq!(rgba.len(), 36 * 36 * 4);
        let alpha = |x: u32, y: u32| rgba[((y * 36 + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0); // corner: outside the squircle
        assert_eq!(alpha(35, 35), 0);
        assert_eq!(alpha(1, 18), 0); // the 32 px glyph is centred: 2 px margins
        assert!(alpha(2, 18) > 0 && alpha(2, 18) < 255); // its anti-aliased left edge
        assert_eq!(alpha(6, 6), 255); // top-left of the squircle, away from the S
        assert_eq!(alpha(29, 25), 255); // bottom-right of it
        // The S runs through the logo's centre (its waist passes 7 units
        // from it) and its ends are where the SVG puts them.
        assert_eq!(alpha(18, 18), 0);
        let pts = logo_s_points();
        assert_eq!(pts.first().copied(), Some((668.0, 354.0)));
        assert_eq!(pts.last().copied(), Some((348.0, 655.0)));
        assert_eq!(pts.len(), 6 * 101);
        // Every S sample lies inside the squircle, and the samples are
        // dense enough for the stroke test (never more than 4 units apart).
        for (i, (x, y)) in pts.iter().enumerate() {
            assert!(((x - 512.0).abs() / 504.0).powi(4) + ((y - 512.0).abs() / 504.0).powi(4) < 1.0);
            if i > 0 {
                let (px, py) = pts[i - 1];
                assert!(((x - px).powi(2) + (y - py).powi(2)).sqrt() < 4.0, "gap at {i}");
            }
        }
        // The top right end of the S is cut out, the lower left corner of
        // the squircle (below the S's tail) is not.
        assert_eq!(alpha(22, 13), 0);
        assert_eq!(alpha(8, 29), 255);
        assert!(rgba.chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
        // Cached: the same bytes come back.
        assert_eq!(template_icon().rgba(), rgba);
    }

    #[test]
    fn the_recording_icon_is_a_red_dot() {
        let dot = recording_icon();
        assert_eq!((dot.width(), dot.height()), (36, 36));
        let rgba = dot.rgba();
        let alpha = |x: u32, y: u32| rgba[((y * 36 + x) * 4 + 3) as usize];
        // Centre 1.5 px above the middle (DOT_LIFT), radius 9.
        assert_eq!(alpha(18, 17), 255);
        assert_eq!(alpha(18, 8), 255); // top edge row: still inside
        assert_eq!(alpha(18, 26), 0); // just below the dot
        assert_eq!(alpha(18, 6), 0); // just above it
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(&rgba[(18 * 36 + 18) * 4..(18 * 36 + 18) * 4 + 3], &[255, 59, 48]);
    }

    #[test]
    fn the_clock_reads_like_the_bar() {
        assert_eq!(format_elapsed(0), "00:00");
        assert_eq!(format_elapsed(65_999), "01:05");
        assert_eq!(format_elapsed(3_725_000), "1:02:05");
        assert_eq!(timer_text(12_000), "Recording 00:12");
        assert!(now_ms() > 1_700_000_000_000);
    }

    #[test]
    fn set_recording_is_a_no_op_without_a_tray() {
        let app = app();
        // No TrayItems managed in the tests (the tray is never created).
        set_recording(&app, true);
        set_recording(&app, false);
        set_update_available(&app, Some("1.2.3"));
        assert!(app.tray_by_id(TRAY_ID).is_none());
        #[cfg(target_os = "macos")]
        assert!(idle_icon(&app).is_some());
        #[cfg(not(target_os = "macos"))]
        let _ = idle_icon(&app);
    }

    /// The menus themselves can only be built on the main thread (muda), so
    /// the tests cannot exercise `TrayItems::set_update_available`.
    #[test]
    fn the_update_line_and_tooltip_name_the_version() {
        assert_eq!(update_text("1.2.3"), "Update to Socorin 1.2.3…");
        assert_eq!(idle_tooltip(None), "Socorin");
        assert_eq!(idle_tooltip(Some("2.0.0")), "Socorin – version 2.0.0 available");
    }
}
