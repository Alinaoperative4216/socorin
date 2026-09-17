//! Update check and install.
//!
//! Once a day (`CHECK_INTERVAL`) the app reads `VERSION_URL` and compares
//! the `version` in it with its own. A newer one is announced in a small
//! popover under the menu bar / tray icon (`windows::UPDATE`), as an
//! "Update to …" line at the top of the tray menu and in Settings. With
//! "Install updates automatically" on (the default) it is installed straight
//! away instead and the popover shows the progress. Installing goes through
//! the updater plugin: the same manifest names a signed installer per
//! platform (see
//! `scripts/publish-landing.sh`); when it has none for this platform the
//! popover offers the download page instead.
//!
//! Nothing is announced or installed while a capture or a recording runs:
//! the check does not count and is repeated at the next poll.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::{capture, record, settings, tray, windows};

/// The manifest the landing page publishes: `{ "version": "1.2.3", … }`,
/// plus the updater plugin's `platforms` for the installers.
pub const VERSION_URL: &str = "https://socorin.com/version.json";
/// How old the last check may be before a new one is made.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the scheduler looks whether a check is due: a setting
/// switched on later, or a laptop that slept through the day, is picked
/// up without a restart, and a failed check is retried.
const POLL: Duration = Duration::from_secs(60 * 60);
/// First look after launch: let the overlays warm up first.
const STARTUP_DELAY: Duration = Duration::from_secs(20);
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Progress is reported at most every this many bytes.
const PROGRESS_STEP: u64 = 256 * 1024;
/// Event carrying a fresh `Status` to the settings window and the popover.
pub const STATUS_EVENT: &str = "update:status";

/// What the updater is doing right now.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    /// Downloading the new version: `received` of `total` bytes (`total` is
    /// 0 when the server does not say).
    Installing { received: u64, total: u64 },
    /// The install failed; the popover shows `error` and the download page.
    Failed { error: String },
}

#[derive(Default)]
pub struct UpdateState {
    phase: Mutex<Phase>,
    /// First launch after an update: the popover says so once.
    just_updated: AtomicBool,
}

impl UpdateState {
    /// Back to idle (the tests share one app).
    #[cfg(test)]
    pub fn reset(&self) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Idle;
        self.just_updated.store(false, Ordering::SeqCst);
    }
}

/// Everything the settings window and the popover show.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub current_version: String,
    /// A newer version that exists ("" when none is known).
    pub available: String,
    /// Unix time (seconds) of the last successful check, 0 = never.
    pub checked_at: u64,
    pub phase: Phase,
    pub just_updated: bool,
}

pub fn status<R: Runtime>(app: &AppHandle<R>) -> Status {
    let settings = settings::current(app);
    let state = app.state::<UpdateState>();
    let phase = state.phase.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Status {
        current_version: app.package_info().version.to_string(),
        available: settings.update_available,
        checked_at: settings.update_checked_at,
        phase,
        just_updated: state.just_updated.load(Ordering::SeqCst),
    }
}

fn phase<R: Runtime>(app: &AppHandle<R>) -> Phase {
    app.state::<UpdateState>().phase.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_phase<R: Runtime>(app: &AppHandle<R>, phase: Phase) {
    *app.state::<UpdateState>().phase.lock().unwrap_or_else(|e| e.into_inner()) = phase;
    broadcast(app);
}

/// Tell the settings window and the popover what is going on.
fn broadcast<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit(STATUS_EVENT, status(app));
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `candidate` (as the manifest spells it, a leading `v` allowed) is newer
/// than `current`. Anything unparsable is not.
pub fn is_newer(candidate: &str, current: &Version) -> bool {
    parse_version(candidate).map(|v| v > *current).unwrap_or(false)
}

fn parse_version(text: &str) -> Result<Version, String> {
    let cleaned = text.trim().trim_start_matches(['v', 'V']);
    Version::parse(cleaned).map_err(|e| format!("bad version {text:?}: {e}"))
}

/// A daily check is due: the setting is on and the last successful check
/// is a day old (or never happened).
pub fn is_due(settings: &settings::Settings) -> bool {
    settings.check_updates
        && now_secs().saturating_sub(settings.update_checked_at) >= CHECK_INTERVAL.as_secs()
}

/// Bookkeeping at launch: forget an announced version this build already
/// is (or passed), notice that an update was installed since the last run
/// and put the tray menu in the right state.
pub fn on_startup<R: Runtime>(app: &AppHandle<R>) {
    let current = app.package_info().version.clone();
    let mut s = settings::current(app);
    let mut changed = false;
    if !s.update_available.is_empty() && !is_newer(&s.update_available, &current) {
        s.update_available.clear();
        changed = true;
    }
    if s.last_version != current.to_string() {
        if !s.last_version.is_empty() {
            app.state::<UpdateState>().just_updated.store(true, Ordering::SeqCst);
        }
        s.last_version = current.to_string();
        changed = true;
    }
    if changed {
        if let Err(e) = settings::store(app, s.clone()) {
            eprintln!("[update] {e}");
        }
    }
    tray::set_update_available(app, available(&s));
    if app.state::<UpdateState>().just_updated.load(Ordering::SeqCst) {
        show_notice(app);
    }
}

fn available(settings: &settings::Settings) -> Option<&str> {
    (!settings.update_available.is_empty()).then_some(settings.update_available.as_str())
}

/// The manifest URL: `VERSION_URL`, or `SOCORIN_DEBUG_UPDATE_URL` when the
/// debug harness is compiled in (to try the popover against a local server;
/// a shipped build must not be pointed at another server by its environment).
pub fn version_url() -> String {
    if crate::debug::HARNESS {
        if let Some(url) = std::env::var("SOCORIN_DEBUG_UPDATE_URL").ok().filter(|u| !u.is_empty()) {
            return url;
        }
    }
    VERSION_URL.into()
}

/// The daily schedule, on its own thread for the life of the app.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let url = version_url();
        // A harness run wants to see the result right away.
        std::thread::sleep(if url == VERSION_URL { STARTUP_DELAY } else { Duration::from_secs(2) });
        loop {
            tick(&app, &url);
            std::thread::sleep(POLL);
        }
    });
}

/// One round of the schedule: a check when one is due.
pub fn tick<R: Runtime>(app: &AppHandle<R>, url: &str) {
    if !is_due(&settings::current(app)) {
        return;
    }
    if let Err(e) = tauri::async_runtime::block_on(check(app.clone(), url.to_string())) {
        eprintln!("[update] {e}");
    }
}

/// Read the manifest at `url` and compare. A newer version is recorded and
/// announced (or installed, with the setting on) and returned. While a
/// capture or a recording runs nothing is recorded or announced, so the
/// check is repeated at the next poll.
pub async fn check<R: Runtime>(app: AppHandle<R>, url: String) -> Result<Option<String>, String> {
    if matches!(phase(&app), Phase::Installing { .. }) {
        return Err("An update is being installed.".into());
    }
    set_phase(&app, Phase::Checking);
    let result = fetch_latest(&url).await;
    // A failed check is not a failure worth a popover; Settings shows it.
    set_phase(&app, Phase::Idle);
    let latest = result?;
    let newer = latest > app.package_info().version;
    if capture::is_busy(&app) || record::is_recording(&app) {
        return Ok(newer.then(|| latest.to_string()));
    }
    let mut s = settings::current(&app);
    s.update_checked_at = now_secs();
    s.update_available = if newer { latest.to_string() } else { String::new() };
    settings::store(&app, s.clone())?;
    broadcast(&app);
    tray::set_update_available(&app, available(&s));
    if !newer {
        return Ok(None);
    }
    if s.auto_update {
        install_from(&app, &url);
    } else {
        show_notice(&app);
    }
    Ok(Some(latest.to_string()))
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
}

/// The `version` in the manifest at `url`.
async fn fetch_latest(url: &str) -> Result<Version, String> {
    ensure_crypto_provider();
    let client = reqwest::Client::builder()
        .user_agent(concat!("Socorin/", env!("CARGO_PKG_VERSION")))
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Cannot reach {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} answered {}", response.status()));
    }
    let manifest: Manifest = response
        .json()
        .await
        .map_err(|e| format!("{url} is not a version manifest: {e}"))?;
    parse_version(&manifest.version)
}

/// reqwest is built without a default TLS backend (the updater plugin does
/// the same and installs ring on first use); make sure one is there before
/// a client is built.
pub(crate) fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Download and install the announced version through the updater plugin,
/// then relaunch. The popover shows the progress, or the failure.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    install_from(app, &version_url());
}

pub fn install_from<R: Runtime>(app: &AppHandle<R>, url: &str) {
    {
        let state = app.state::<UpdateState>();
        let mut phase = state.phase.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*phase, Phase::Installing { .. }) {
            return;
        }
        *phase = Phase::Installing { received: 0, total: 0 };
    }
    broadcast(app);
    show_notice(app);
    let app = app.clone();
    let url = url.to_string();
    std::thread::spawn(move || {
        match tauri::async_runtime::block_on(download_and_install(&app, &url)) {
            Ok(version) => {
                // macOS / Linux: the new bundle is in place. (Windows never
                // gets here: the installer relaunches the app itself.)
                eprintln!("[update] installed {version}, restarting");
                app.restart();
            }
            Err(e) => {
                eprintln!("[update] {e}");
                set_phase(&app, Phase::Failed { error: e });
                show_notice(&app);
            }
        }
    });
}

async fn download_and_install<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<String, String> {
    let endpoint: url::Url = url.parse().map_err(|e: url::ParseError| e.to_string())?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(describe)?
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(describe)?;
    let mut update = updater
        .check()
        .await
        .map_err(describe)?
        .ok_or_else(|| "Socorin is already up to date.".to_string())?;
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    let progress = app.clone();
    let mut received = 0u64;
    let mut reported = 0u64;
    update
        .download_and_install(
            |chunk, total| {
                received += chunk as u64;
                if received - reported >= PROGRESS_STEP {
                    reported = received;
                    set_phase(&progress, Phase::Installing { received, total: total.unwrap_or(0) });
                }
            },
            || {},
        )
        .await
        .map_err(describe)?;
    Ok(update.version.clone())
}

/// Plugin errors in the words the popover shows. A manifest without an
/// installer for this platform (no `platforms` at all, which the plugin
/// reports as a deserialization error, or none for this target) is not a
/// failure of the app: the new version is just not available this way.
fn describe(e: tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error;
    match e {
        Error::TargetNotFound(_) | Error::TargetsNotFound(_) | Error::Serialization(_) | Error::ReleaseNotFound => {
            "There is no automatic update for this platform yet. Get the new version from socorin.com.".into()
        }
        e => e.to_string(),
    }
}

fn show_notice<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = windows::show_update_notice(app) {
        eprintln!("[update] {e}");
    }
}

/// "Later", Escape, OK or a click elsewhere: hide the popover. A failure is
/// forgotten; the announced version stays (tray menu and Settings keep it).
pub fn dismiss<R: Runtime>(app: &AppHandle<R>) {
    app.state::<UpdateState>().just_updated.store(false, Ordering::SeqCst);
    if matches!(phase(app), Phase::Failed { .. }) {
        set_phase(app, Phase::Idle);
    }
    windows::hide_update_notice(app);
}

/// The popover lost focus: it goes away, unless it is showing progress.
pub fn blurred<R: Runtime>(app: &AppHandle<R>) {
    if !matches!(phase(app), Phase::Installing { .. }) {
        dismiss(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        settings::Settings,
        test_support::{app_with, geom, settings_in, shot, start_session, temp_dir},
    };
    use std::{
        future::Future,
        io::{Read, Write},
        net::TcpListener,
    };

    /// A tiny HTTP server answering `routes` (path → status, body) for the
    /// rest of the test process. Returns its base URL.
    fn serve(routes: Vec<(&'static str, u16, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, ..)| *p == path)
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, b"not here".to_vec()));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    fn manifest(version: &str) -> Vec<u8> {
        format!(r#"{{ "version": "{version}", "notes": "x" }}"#).into_bytes()
    }

    /// A manifest the updater plugin accepts for this platform, pointing at
    /// `pkg_url` with a signature that cannot be decoded.
    fn full_manifest(version: &str, pkg_url: &str) -> Vec<u8> {
        let os = if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        };
        format!(
            r#"{{ "version": "{version}", "platforms": {{ "{os}-{}": {{ "url": "{pkg_url}", "signature": "bm9wZQ==" }} }} }}"#,
            std::env::consts::ARCH
        )
        .into_bytes()
    }

    fn run<F: Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn wait_for<R: Runtime>(app: &AppHandle<R>, pred: impl Fn(&Phase) -> bool) -> Phase {
        for _ in 0..600 {
            let p = phase(app);
            if pred(&p) {
                return p;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        phase(app)
    }

    #[test]
    fn versions_compare_as_semver() {
        assert!(is_newer("0.1.1", &v("0.1.0")));
        assert!(is_newer("v1.0.0", &v("0.9.9")));
        assert!(is_newer(" 2.0.0 ", &v("1.10.0")));
        assert!(!is_newer("0.1.0", &v("0.1.0")));
        assert!(!is_newer("0.0.9", &v("0.1.0")));
        assert!(!is_newer("latest", &v("0.1.0")));
        assert!(!is_newer("", &v("0.1.0")));
        assert!(parse_version("1.2").is_err());
        assert!(now_secs() > 1_700_000_000);
    }

    #[test]
    fn the_manifest_url_can_be_overridden_by_the_harness_only() {
        assert_eq!(version_url(), VERSION_URL);
        std::env::set_var("SOCORIN_DEBUG_UPDATE_URL", "");
        assert_eq!(version_url(), VERSION_URL);
        std::env::set_var("SOCORIN_DEBUG_UPDATE_URL", "http://127.0.0.1:9/v.json");
        assert_eq!(version_url(), if crate::debug::HARNESS { "http://127.0.0.1:9/v.json" } else { VERSION_URL });
        std::env::remove_var("SOCORIN_DEBUG_UPDATE_URL");
        assert_eq!(version_url(), VERSION_URL);
    }

    #[test]
    fn a_check_is_due_once_a_day_and_only_when_enabled() {
        let never = Settings::default();
        assert!(is_due(&never));
        let fresh = Settings { update_checked_at: now_secs(), ..Settings::default() };
        assert!(!is_due(&fresh));
        let yesterday = Settings { update_checked_at: now_secs() - CHECK_INTERVAL.as_secs() - 1, ..Settings::default() };
        assert!(is_due(&yesterday));
        let off = Settings { check_updates: false, ..Settings::default() };
        assert!(!is_due(&off));
        let future = Settings { update_checked_at: now_secs() + 1_000, ..Settings::default() };
        assert!(!is_due(&future));
    }

    #[test]
    fn a_newer_version_is_recorded_and_announced() {
        let dir = temp_dir("update-newer");
        // Automatic installs off: the version is announced, not installed.
        let app = app_with(Settings { auto_update: false, ..settings_in(&dir) });
        let handle = app.handle().clone();
        let base = serve(vec![("/version.json", 200, manifest("0.1.1"))]);
        let before = now_secs();
        let found = run(check(handle.clone(), format!("{base}/version.json"))).unwrap();
        assert_eq!(found.as_deref(), Some("0.1.1"));
        let s = settings::current(&handle);
        assert_eq!(s.update_available, "0.1.1");
        assert!(s.update_checked_at >= before);
        assert!(app.get_webview_window(windows::UPDATE).is_some(), "the popover window exists");
        let status = status(&handle);
        assert_eq!(status.available, "0.1.1");
        assert_eq!(status.phase, Phase::Idle);
        assert_eq!(status.current_version, handle.package_info().version.to_string());
        assert!(!status.just_updated);

        // The same version as ours: the announcement is withdrawn.
        let base = serve(vec![("/version.json", 200, manifest("v0.1.0"))]);
        assert_eq!(run(check(handle.clone(), format!("{base}/version.json"))).unwrap(), None);
        assert_eq!(settings::current(&handle).update_available, "");
        dismiss(&handle);
    }

    #[test]
    fn failed_checks_do_not_count() {
        let dir = temp_dir("update-fail");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        let base = serve(vec![
            ("/500", 500, manifest("9.9.9")),
            ("/text", 200, b"<html>".to_vec()),
            ("/bad", 200, manifest("soon")),
        ]);
        for path in ["/500", "/text", "/bad", "/missing"] {
            let err = run(check(handle.clone(), format!("{base}{path}"))).unwrap_err();
            assert!(!err.is_empty(), "{path}");
            assert_eq!(phase(&handle), Phase::Idle);
        }
        // Nothing listening on port 9.
        let err = run(check(handle.clone(), "http://127.0.0.1:9/version.json".into())).unwrap_err();
        assert!(err.starts_with("Cannot reach"), "{err}");
        assert_eq!(settings::current(&handle).update_checked_at, 0);
        assert_eq!(settings::current(&handle).update_available, "");
    }

    #[test]
    fn nothing_is_announced_while_capturing() {
        let dir = temp_dir("update-busy");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        start_session(&app, vec![shot(geom(1, 0, 0, 8, 8, 1.0, true))]);
        let base = serve(vec![("/version.json", 200, manifest("0.1.9"))]);
        let found = run(check(handle.clone(), format!("{base}/version.json"))).unwrap();
        assert_eq!(found.as_deref(), Some("0.1.9"));
        let s = settings::current(&handle);
        assert_eq!((s.update_checked_at, s.update_available.as_str()), (0, ""));
        capture::cancel(&handle);
    }

    #[test]
    fn automatic_installs_report_their_failure_in_the_popover() {
        let dir = temp_dir("update-auto");
        // Automatic installs are the default: nothing to switch on.
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        assert!(settings::current(&handle).auto_update, "on by default");
        // The manifest is served twice: by our check and by the plugin's.
        let base = serve(vec![("/pkg", 200, b"not an installer".to_vec())]);
        let base = serve(vec![("/version.json", 200, full_manifest("0.1.1", &format!("{base}/pkg")))]);
        let found = run(check(handle.clone(), format!("{base}/version.json"))).unwrap();
        assert_eq!(found.as_deref(), Some("0.1.1"));
        let outcome = wait_for(&handle, |p| matches!(p, Phase::Failed { .. }));
        let Phase::Failed { error } = outcome else { panic!("{outcome:?}") };
        assert!(!error.is_empty());
        // A check is refused while installing, but not while failed.
        assert_eq!(status(&handle).available, "0.1.1");
        blurred(&handle); // click elsewhere: the failure is forgotten
        assert_eq!(status(&handle).phase, Phase::Idle);
        assert_eq!(status(&handle).available, "0.1.1");

        // No installer for this platform in the manifest (none at all, or
        // only for others): says so instead of failing.
        let others = br#"{ "version": "0.1.2", "platforms": { "beos-m68k": { "url": "http://127.0.0.1:9/x", "signature": "bm9wZQ==" } } }"#.to_vec();
        for body in [manifest("0.1.2"), others] {
            let base = serve(vec![("/version.json", 200, body)]);
            install_from(&handle, &format!("{base}/version.json"));
            let Phase::Failed { error } = wait_for(&handle, |p| matches!(p, Phase::Failed { .. })) else { panic!() };
            assert!(error.contains("no automatic update"), "{error}");
            dismiss(&handle);
        }
        assert!(!run(check(handle.clone(), "http://127.0.0.1:9/".into())).unwrap_err().is_empty());

        // Already up to date according to the plugin (same version).
        let base = serve(vec![("/version.json", 200, full_manifest("0.1.0", "http://127.0.0.1:9/pkg"))]);
        install_from(&handle, &format!("{base}/version.json"));
        let Phase::Failed { error } = wait_for(&handle, |p| matches!(p, Phase::Failed { .. })) else { panic!() };
        assert!(error.contains("up to date"), "{error}");
        dismiss(&handle);
        assert_eq!(phase(&handle), Phase::Idle);
    }

    #[test]
    fn installing_is_exclusive_and_refuses_checks() {
        let dir = temp_dir("update-exclusive");
        let app = app_with(settings_in(&dir));
        let handle = app.handle().clone();
        set_phase(&handle, Phase::Installing { received: 5, total: 10 });
        install_from(&handle, "http://127.0.0.1:9/version.json"); // ignored: no second install
        assert_eq!(phase(&handle), Phase::Installing { received: 5, total: 10 });
        let err = run(check(handle.clone(), "http://127.0.0.1:9/".into())).unwrap_err();
        assert!(err.contains("being installed"), "{err}");
        blurred(&handle); // the popover stays while installing
        assert_eq!(phase(&handle), Phase::Installing { received: 5, total: 10 });
        set_phase(&handle, Phase::Idle);
        let json = serde_json::to_value(status(&handle)).unwrap();
        assert_eq!(json["phase"], serde_json::json!({ "phase": "idle" }));
        assert!(json.get("currentVersion").is_some() && json.get("checkedAt").is_some());
        let failed = serde_json::to_value(Phase::Failed { error: "x".into() }).unwrap();
        assert_eq!(failed, serde_json::json!({ "phase": "failed", "error": "x" }));
    }

    #[test]
    fn startup_forgets_stale_announcements_and_notices_an_update() {
        let dir = temp_dir("update-startup");
        let current = Settings::default().last_version; // ""
        assert_eq!(current, "");
        let app = app_with(Settings {
            update_available: "0.0.1".into(), // older than this build: stale
            last_version: "0.0.9".into(),
            ..settings_in(&dir)
        });
        let handle = app.handle().clone();
        on_startup(&handle);
        let s = settings::current(&handle);
        assert_eq!(s.update_available, "");
        assert_eq!(s.last_version, handle.package_info().version.to_string());
        assert!(status(&handle).just_updated);
        dismiss(&handle);
        assert!(!status(&handle).just_updated);

        // First run ever: nothing to say; a real announcement is kept.
        *handle.state::<settings::SettingsState>().0.lock().unwrap() =
            Settings { update_available: "99.0.0".into(), ..settings_in(&dir) };
        on_startup(&handle);
        assert!(!status(&handle).just_updated);
        let s = settings::current(&handle);
        assert_eq!(s.update_available, "99.0.0");
        assert_eq!(s.last_version, handle.package_info().version.to_string());
        on_startup(&handle); // nothing changes, nothing is written
        assert!(!status(&handle).just_updated);
    }

    #[test]
    fn the_scheduler_only_checks_when_due() {
        let dir = temp_dir("update-tick");
        let app = app_with(Settings { check_updates: false, ..settings_in(&dir) });
        let handle = app.handle().clone();
        let base = serve(vec![("/version.json", 200, manifest("0.1.5"))]);
        tick(&handle, &format!("{base}/version.json"));
        assert_eq!(settings::current(&handle).update_available, "");
        *handle.state::<settings::SettingsState>().0.lock().unwrap() = Settings { auto_update: false, ..settings_in(&dir) };
        tick(&handle, "http://127.0.0.1:9/version.json"); // fails, logged
        assert_eq!(settings::current(&handle).update_checked_at, 0);
        tick(&handle, &format!("{base}/version.json"));
        assert_eq!(settings::current(&handle).update_available, "0.1.5");
        tick(&handle, "http://127.0.0.1:9/version.json"); // not due any more: no request
        assert_eq!(settings::current(&handle).update_available, "0.1.5");
        dismiss(&handle);
    }
}
