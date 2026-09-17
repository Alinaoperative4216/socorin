//! Shared helpers for the unit tests: the app on Tauri's mock runtime with
//! the same plugins, state and commands as the real one (see
//! `crate::configure`), plus a private `HOME` so the tests never touch the
//! user's settings, pictures or launch agents.
//!
//! What the tests deliberately never do on this machine: ask for the Screen
//! Recording permission (a system prompt), grab the screen, activate the app
//! (steals focus), write the clipboard, open Finder / System Settings or a
//! save dialog, or start a real recording.

use std::{
    ops::Deref,
    path::PathBuf,
    sync::{atomic::Ordering, Mutex, MutexGuard, Once, OnceLock},
};

use tauri::{
    test::{mock_builder, mock_context, noop_assets, MockRuntime},
    AppHandle, Manager,
};
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use xcap::image::{Rgba, RgbaImage};

use crate::{
    capture::{AppState, MonitorGeom, MonitorShot},
    record::RecordState,
    settings::{Settings, SettingsState},
};

static HOME: Once = Once::new();

/// The app's real updater public key (any valid minisign key would do: the
/// tests never have a matching signature, so installs always fail).
const UPDATER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEVFMDdBNTM2RTYxNDU1MjcKUldRblZSVG1OcVVIN29MelNtTjBjRlFqK29aOEpBM21XLzVqZHFQdHZLenlleENqdzlpK25KYWUK";

/// A scratch directory that stands in for the user's home for the whole test
/// process (`dirs` reads `$HOME` on every call).
pub fn private_home() -> PathBuf {
    HOME.call_once(|| {
        let dir = std::env::temp_dir().join(format!("screenshot-app-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp home");
        std::env::set_var("HOME", &dir);
    });
    PathBuf::from(std::env::var_os("HOME").expect("HOME"))
}

/// A fresh, empty directory under the private home.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = private_home().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Default settings that save into `dir` and never touch the clipboard.
pub fn settings_in(dir: &std::path::Path) -> Settings {
    Settings {
        save_dir: dir.to_string_lossy().into_owned(),
        copy_on_save: false,
        ..Settings::default()
    }
}

/// The global-shortcut plugin can only exist once per process (it installs
/// a system-wide hook), so every test shares one app and takes this lock.
static LOCK: Mutex<()> = Mutex::new(());
static APP: OnceLock<AppHandle<MockRuntime>> = OnceLock::new();

/// Exclusive access to the shared mock app, with fresh state.
pub struct TestApp {
    _guard: MutexGuard<'static, ()>,
    handle: AppHandle<MockRuntime>,
}

impl TestApp {
    pub fn handle(&self) -> &AppHandle<MockRuntime> {
        &self.handle
    }
}

impl Deref for TestApp {
    type Target = AppHandle<MockRuntime>;
    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

pub fn app() -> TestApp {
    app_with(Settings::default())
}

/// The configured app on the mock runtime, with `settings` as the current
/// settings (the real app loads them in `setup`), no capture session, no
/// recording and no shortcuts. Windows created by earlier tests stay.
pub fn app_with(settings: Settings) -> TestApp {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let handle = APP
        .get_or_init(|| {
            private_home();
            let mut context = mock_context(noop_assets());
            context.config_mut().identifier = "com.socorin.test".into();
            // The updater plugin refuses to start without its config; the
            // real one is in tauri.conf.json (the mock context has none).
            context.config_mut().plugins.0.insert(
                "updater".into(),
                serde_json::json!({ "pubkey": UPDATER_PUBKEY, "endpoints": [] }),
            );
            let app = crate::configure(mock_builder()).build(context).expect("mock app");
            app.manage(SettingsState(Mutex::new(Settings::default())));
            let handle = app.handle().clone();
            // Never dropped: the tests only ever use the handle.
            std::mem::forget(app);
            handle
        })
        .clone();

    *handle.state::<SettingsState>().0.lock().unwrap() = settings;
    let capture = handle.state::<AppState>();
    capture.shots.lock().unwrap().clear();
    *capture.pending.lock().unwrap() = None;
    *capture.started.lock().unwrap() = None;
    capture.busy.store(false, Ordering::SeqCst);
    capture.annotating.store(false, Ordering::SeqCst);
    capture.record_mode.store(false, Ordering::SeqCst);
    if let Some(active) = handle.state::<RecordState>().active.lock().unwrap().take() {
        active.backend.abandon();
    }
    let _ = handle.global_shortcut().unregister_all();
    handle.state::<crate::update::UpdateState>().reset();
    TestApp { _guard: guard, handle }
}

pub fn geom(id: u32, x: i32, y: i32, width: u32, height: u32, scale: f32, primary: bool) -> MonitorGeom {
    MonitorGeom {
        id,
        name: format!("Display {id}"),
        x,
        y,
        width,
        height,
        scale,
        primary,
    }
}

/// A bitmap whose pixels encode their own position (so crops can be checked).
pub fn image(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255]))
}

pub fn shot(geom: MonitorGeom) -> MonitorShot {
    let scale = geom.scale.max(0.1);
    let image = image(
        (geom.width as f32 * scale).round() as u32,
        (geom.height as f32 * scale).round() as u32,
    );
    MonitorShot { geom, image }
}

/// Make `shots` the bitmaps of a running session (as `begin_region_inner`
/// would after grabbing the screen).
pub fn start_session(app: &AppHandle<MockRuntime>, shots: Vec<MonitorShot>) -> u64 {
    let state = app.state::<AppState>();
    state.busy.store(true, Ordering::SeqCst);
    let session = state.session.fetch_add(1, Ordering::SeqCst) + 1;
    *state.started.lock().unwrap() = Some(std::time::Instant::now());
    *state.shots.lock().unwrap() = shots;
    session
}

/// A child process that lives until it is signalled, standing in for a
/// recording encoder.
pub fn sleeping_child() -> std::process::Child {
    std::process::Command::new("sleep")
        .arg("30")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("sleep")
}
