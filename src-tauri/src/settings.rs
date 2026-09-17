//! Persistent user settings (JSON file in the app config dir).

use std::{fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AfterCapture {
    /// Open the annotation editor (default).
    Editor,
    /// Copy straight to the clipboard.
    Clipboard,
    /// Save straight to the save directory.
    Save,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Global shortcut for region capture, e.g. "CmdOrCtrl+Shift+A".
    pub hotkey: String,
    /// Global shortcut for full-screen capture. Empty = disabled.
    pub fullscreen_hotkey: String,
    /// Global shortcut that starts a region recording / stops the running
    /// one. Empty = disabled.
    pub record_hotkey: String,
    /// Directory where quick-saved screenshots go.
    pub save_dir: String,
    /// What happens right after a region is selected.
    pub after_capture: AfterCapture,
    /// Also copy to clipboard whenever a file is saved.
    pub copy_on_save: bool,
    /// Launch at login. On by default, until the user switches it off.
    pub autostart: bool,
    /// Honour `--capture`, `--capture-full`, `--record` and `--record-full`
    /// on the command line (needed for desktop-environment shortcuts on
    /// Wayland). Off by default: any local program could otherwise make this
    /// app, which holds the screen-recording permission, capture the screen
    /// on its behalf.
    pub cli_triggers: bool,
    /// File name prefix for saved screenshots.
    pub file_prefix: String,
    /// Linux: the ffmpeg binary used for recording; empty = look in the
    /// usual install locations, then on PATH. Windows: empty = the built-in
    /// recorder, a path = record with that ffmpeg instead.
    pub ffmpeg_path: String,
    /// The first-launch welcome dialog has been dismissed.
    pub welcome_shown: bool,
    /// Last annotation colour (#rrggbb) and stroke level (1..=5), remembered
    /// across captures.
    pub annotation_color: String,
    pub annotation_stroke: u8,
    /// Look for a newer version on socorin.com once a day. On by default.
    pub check_updates: bool,
    /// Install a newer version as soon as the daily check finds one. Off by
    /// default: the user is only told.
    pub auto_update: bool,
    /// Unix time (seconds) of the last successful update check, 0 = never.
    pub update_checked_at: u64,
    /// Newer version the last check found ("" = none), so the tray menu and
    /// Settings keep offering it across restarts.
    pub update_available: String,
    /// Version that ran last time; a different one now means an update was
    /// installed since (the popover says so once).
    pub last_version: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+A".into(),
            fullscreen_hotkey: String::new(),
            record_hotkey: String::new(),
            save_dir: String::new(),
            after_capture: AfterCapture::Editor,
            copy_on_save: true,
            autostart: true,
            cli_triggers: false,
            file_prefix: DEFAULT_PREFIX.into(),
            ffmpeg_path: String::new(),
            welcome_shown: false,
            annotation_color: DEFAULT_COLOR.into(),
            annotation_stroke: DEFAULT_STROKE,
            check_updates: true,
            auto_update: false,
            update_checked_at: 0,
            update_available: String::new(),
            last_version: String::new(),
        }
    }
}

pub struct SettingsState(pub Mutex<Settings>);

pub const DEFAULT_COLOR: &str = "#ff3b30";
pub const DEFAULT_STROKE: u8 = 3;
pub const MAX_STROKE: u8 = 5;
pub const DEFAULT_PREFIX: &str = "Socorin";
/// Leaves room for the time stamp and extension on every file system.
const MAX_PREFIX_LEN: usize = 64;

/// A file-name prefix must stay one plain file name inside the save folder.
/// Path separators, the characters Windows rejects and control characters
/// are dropped; leading / trailing dots go too (so `..` cannot climb out and
/// nothing becomes a hidden file); an empty result falls back to the default.
pub fn sanitise_prefix(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(MAX_PREFIX_LEN)
        .collect();
    let kept = kept.trim().trim_matches('.').trim();
    if kept.is_empty() {
        DEFAULT_PREFIX.into()
    } else {
        kept.to_string()
    }
}

/// Fill in anything empty or out of range.
pub fn normalise<R: Runtime>(app: &AppHandle<R>, settings: &mut Settings) {
    if settings.save_dir.trim().is_empty() {
        settings.save_dir = default_save_dir(app).to_string_lossy().into_owned();
    }
    settings.file_prefix = sanitise_prefix(&settings.file_prefix);
    settings.ffmpeg_path = settings.ffmpeg_path.trim().to_string();
    let color = settings.annotation_color.trim().to_ascii_lowercase();
    let valid_color = color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    settings.annotation_color = if valid_color { color } else { DEFAULT_COLOR.into() };
    settings.annotation_stroke = settings.annotation_stroke.clamp(1, MAX_STROKE);
    settings.update_available = settings.update_available.trim().to_string();
}

fn config_file<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("settings.json"))
}

pub fn default_save_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    app.path()
        .picture_dir()
        .or_else(|_| app.path().home_dir())
        .map(|d| d.join("Screenshots"))
        .unwrap_or_else(|_| PathBuf::from("Screenshots"))
}

/// The folder quick saves and recordings go to (the setting, or the default
/// when it is empty).
pub fn save_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let configured = current(app).save_dir;
    let configured = configured.trim();
    if configured.is_empty() {
        default_save_dir(app)
    } else {
        PathBuf::from(configured)
    }
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Settings {
    let mut settings = config_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default();
    normalise(app, &mut settings);
    settings
}

/// Writes the whole file to a temporary name first and renames it over the
/// old one, so a crash mid-write leaves the previous settings rather than a
/// truncated file (which `load` would silently replace with the defaults —
/// autostart on, the command-line triggers off).
pub fn save<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), String> {
    let path = config_file(app).ok_or("cannot resolve config dir")?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    write_atomically(&path, json.as_bytes())
}

fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("settings path has no directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(&format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = fs::write(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Snapshot of the current settings.
pub fn current<R: Runtime>(app: &AppHandle<R>) -> Settings {
    app.state::<SettingsState>().0.lock().unwrap().clone()
}

/// Persist `settings` and make them current.
pub fn store<R: Runtime>(app: &AppHandle<R>, settings: Settings) -> Result<(), String> {
    save(app, &settings)?;
    *app.state::<SettingsState>().0.lock().unwrap() = settings;
    Ok(())
}

/// Apply a partial update (camelCase keys, as the webviews send them) on top
/// of `base`. Each window only owns a few settings, so a full replace would
/// let a stale copy clobber what another window just saved.
pub fn merge(
    base: &Settings,
    patch: &serde_json::Map<String, serde_json::Value>,
) -> Result<Settings, String> {
    let mut json = serde_json::to_value(base).map_err(|e| e.to_string())?;
    let obj = json.as_object_mut().ok_or("settings are not an object")?;
    for (key, value) in patch {
        obj.insert(key.clone(), value.clone());
    }
    serde_json::from_value(json).map_err(|e| format!("invalid settings: {e}"))
}

pub fn mark_welcome_shown<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let mut settings = current(app);
    if settings.welcome_shown {
        return Ok(());
    }
    settings.welcome_shown = true;
    store(app, settings)
}

#[cfg(test)]
mod sanitise_tests {
    use super::{sanitise_prefix, DEFAULT_PREFIX};

    #[test]
    fn keeps_ordinary_prefixes() {
        assert_eq!(sanitise_prefix("Socorin"), "Socorin");
        assert_eq!(sanitise_prefix("Screenshot"), "Screenshot");
        assert_eq!(sanitise_prefix("  shot v1.2 "), "shot v1.2");
        assert_eq!(sanitise_prefix("ảnh màn hình"), "ảnh màn hình");
    }

    #[test]
    fn cannot_leave_the_save_folder() {
        assert_eq!(sanitise_prefix("../../Desktop/x"), "Desktopx");
        assert_eq!(sanitise_prefix(".."), DEFAULT_PREFIX);
        assert_eq!(sanitise_prefix("/etc/passwd"), "etcpasswd");
        assert_eq!(sanitise_prefix("C:\\Users\\me"), "CUsersme");
        assert_eq!(sanitise_prefix("..\\..\\x"), "x");
    }

    #[test]
    fn drops_hidden_and_windows_reserved_characters() {
        assert_eq!(sanitise_prefix(".hidden"), "hidden");
        assert_eq!(sanitise_prefix("a<b>c:d\"e|f?g*h"), "abcdefgh");
        assert_eq!(sanitise_prefix("tab\there\n"), "tabhere");
        assert_eq!(sanitise_prefix(""), DEFAULT_PREFIX);
        assert_eq!(sanitise_prefix("   "), DEFAULT_PREFIX);
    }

    #[test]
    fn is_bounded() {
        assert_eq!(sanitise_prefix(&"x".repeat(500)).len(), 64);
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;
    use crate::test_support::{app, app_with, private_home, settings_in, temp_dir};
    use serde_json::json;

    fn patch(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn serialises_with_camel_case_keys_and_lowercase_modes() {
        let json = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(json["afterCapture"], "editor");
        assert_eq!(json["copyOnSave"], true);
        assert_eq!(json["annotationStroke"], 3);
        assert_eq!(json["filePrefix"], "Socorin");
        assert_eq!(json["autostart"], true);
        let back: Settings = serde_json::from_str(r#"{"afterCapture":"save","hotkey":"F5"}"#).unwrap();
        assert_eq!(back.after_capture, AfterCapture::Save);
        assert_eq!(back.hotkey, "F5");
        assert_eq!(back.file_prefix, DEFAULT_PREFIX);
    }

    #[test]
    fn launch_at_login_is_on_until_the_user_turns_it_off() {
        assert!(Settings::default().autostart);
        // A settings file from before the switch existed: still on.
        assert!(serde_json::from_str::<Settings>("{}").unwrap().autostart);
        // The user's choice is kept.
        assert!(!serde_json::from_str::<Settings>(r#"{"autostart":false}"#).unwrap().autostart);
    }

    #[test]
    fn update_checks_are_on_and_automatic_installs_off_by_default() {
        let d = Settings::default();
        assert!(d.check_updates);
        assert!(!d.auto_update);
        assert_eq!((d.update_checked_at, d.update_available.as_str(), d.last_version.as_str()), (0, "", ""));
        // A settings file from before the feature existed: same defaults.
        let old: Settings = serde_json::from_str(r#"{"hotkey":"F5"}"#).unwrap();
        assert!(old.check_updates && !old.auto_update);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["checkUpdates"], true);
        assert_eq!(json["autoUpdate"], false);
        assert_eq!(json["updateCheckedAt"], 0);
        assert_eq!(json["updateAvailable"], "");
        let chosen: Settings = serde_json::from_str(r#"{"checkUpdates":false,"autoUpdate":true,"updateAvailable":" 1.2.3 "}"#).unwrap();
        assert!(!chosen.check_updates && chosen.auto_update);
        let app = app();
        let mut s = chosen;
        normalise(app.handle(), &mut s);
        assert_eq!(s.update_available, "1.2.3");
    }

    #[test]
    fn merge_applies_only_the_given_keys() {
        let base = Settings {
            annotation_color: "#123456".into(),
            ..Settings::default()
        };
        let merged = merge(&base, &patch(json!({ "hotkey": "F6", "afterCapture": "clipboard" }))).unwrap();
        assert_eq!(merged.hotkey, "F6");
        assert_eq!(merged.after_capture, AfterCapture::Clipboard);
        assert_eq!(merged.annotation_color, "#123456");
        let err = merge(&base, &patch(json!({ "annotationStroke": "thick" }))).unwrap_err();
        assert!(err.starts_with("invalid settings"), "{err}");
    }

    #[test]
    fn normalise_fills_in_and_clamps() {
        let app = app();
        let mut s = Settings {
            save_dir: "  ".into(),
            file_prefix: "../x".into(),
            ffmpeg_path: " /opt/ffmpeg ".into(),
            annotation_color: " #ABCDEF ".into(),
            annotation_stroke: 0,
            ..Settings::default()
        };
        normalise(app.handle(), &mut s);
        assert_eq!(s.save_dir, private_home().join("Pictures").join("Screenshots").to_string_lossy());
        assert_eq!(s.file_prefix, "x");
        assert_eq!(s.ffmpeg_path, "/opt/ffmpeg");
        assert_eq!(s.annotation_color, "#abcdef");
        assert_eq!(s.annotation_stroke, 1);

        s.annotation_color = "red".into();
        s.annotation_stroke = 200;
        normalise(app.handle(), &mut s);
        assert_eq!(s.annotation_color, DEFAULT_COLOR);
        assert_eq!(s.annotation_stroke, MAX_STROKE);
    }

    #[test]
    fn loads_defaults_without_a_file_and_round_trips_through_disk() {
        let app = app();
        let handle = app.handle();
        let loaded = load(handle);
        assert_eq!(loaded.hotkey, "CmdOrCtrl+Shift+A");
        assert!(!loaded.save_dir.is_empty());

        let mut wanted = Settings::default();
        wanted.hotkey = "Ctrl+Alt+F17".into();
        wanted.welcome_shown = true;
        save(handle, &wanted).unwrap();
        let path = config_file(handle).unwrap();
        assert!(path.ends_with("settings.json"));
        assert!(path.starts_with(private_home()));
        let again = load(handle);
        assert_eq!(again.hotkey, "Ctrl+Alt+F17");
        assert!(again.welcome_shown);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load(handle).hotkey, "CmdOrCtrl+Shift+A");
    }

    /// The file is replaced in one step: no temporary file is left behind,
    /// and a write that cannot finish leaves the old contents intact.
    #[test]
    fn saving_replaces_the_file_atomically() {
        let dir = temp_dir("settings-atomic");
        let path = dir.join("settings.json");
        write_atomically(&path, b"first").unwrap();
        write_atomically(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("settings.json")]);

        // The target's directory is a file: nothing can be written there.
        let blocked = path.join("settings.json");
        assert!(write_atomically(&blocked, b"third").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(write_atomically(std::path::Path::new("/"), b"x").is_err());
    }

    #[test]
    fn store_and_current_share_the_managed_state() {
        let dir = temp_dir("settings-store");
        let app = app_with(settings_in(&dir));
        let handle = app.handle();
        assert_eq!(save_dir(handle), dir);
        assert!(!current(handle).welcome_shown);

        mark_welcome_shown(handle).unwrap();
        assert!(current(handle).welcome_shown);
        let stamp = std::fs::metadata(config_file(handle).unwrap()).unwrap().modified().unwrap();
        // Already shown: nothing is written again.
        mark_welcome_shown(handle).unwrap();
        assert_eq!(std::fs::metadata(config_file(handle).unwrap()).unwrap().modified().unwrap(), stamp);

        store(handle, Settings { save_dir: "".into(), ..current(handle) }).unwrap();
        assert_eq!(save_dir(handle), default_save_dir(handle));
        assert_eq!(default_save_dir(handle), private_home().join("Pictures").join("Screenshots"));
    }
}
