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
    /// Upload to the share server and put the link on the clipboard.
    Upload,
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
    /// Install a newer version as soon as the daily check finds one. On by
    /// default; a settings file that says `false` keeps it off, so a user
    /// who switched it off stays in charge.
    pub auto_update: bool,
    /// Unix time (seconds) of the last successful update check, 0 = never.
    pub update_checked_at: u64,
    /// Newer version the last check found ("" = none), so the tray menu and
    /// Settings keep offering it across restarts.
    pub update_available: String,
    /// Version that ran last time; a different one now means an update was
    /// installed since (the popover says so once).
    pub last_version: String,
    /// Where "Upload & copy link" sends captures (`share.rs`): an http(s)
    /// origin without a trailing slash, socorin.com by default.
    pub upload_server: String,
    /// The random id the share server gave this install (its spam control;
    /// there are no accounts). Empty until the first upload registers one;
    /// "Reset install ID" in Settings empties it again.
    pub install_id: String,
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
            auto_update: true,
            update_checked_at: 0,
            update_available: String::new(),
            last_version: String::new(),
            upload_server: DEFAULT_UPLOAD_SERVER.into(),
            install_id: String::new(),
        }
    }
}

pub struct SettingsState(pub Mutex<Settings>);

pub const DEFAULT_COLOR: &str = "#ff3b30";
pub const DEFAULT_STROKE: u8 = 3;
pub const MAX_STROKE: u8 = 5;
pub const DEFAULT_PREFIX: &str = "Socorin";
pub const DEFAULT_UPLOAD_SERVER: &str = "https://socorin.com";
/// Leaves room for the time stamp and extension on every file system.
const MAX_PREFIX_LEN: usize = 64;

/// The share server as an origin: `http://` or `https://` with a host, an
/// optional port and no path, query or trailing slash (the API paths are
/// appended to it). `None` when the text is not such an address, so the
/// settings window can say so instead of silently saving something else.
pub fn sanitise_server_checked(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = url::Url::parse(trimmed).ok()?;
    let plain = matches!(url.scheme(), "http" | "https")
        && url.host_str().map_or(false, |h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/");
    if !plain {
        return None;
    }
    // `Url` lower-cases the scheme and host; the port stays when it is not
    // the scheme's default.
    let mut origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    Some(origin)
}

/// The share server, falling back to socorin.com for anything that is not
/// an origin (an empty field, a settings file edited by hand).
pub fn sanitise_server(raw: &str) -> String {
    sanitise_server_checked(raw).unwrap_or_else(|| DEFAULT_UPLOAD_SERVER.into())
}

/// The install id is opaque to the app; only obviously broken values (spaces,
/// control characters, absurd length) are dropped so a corrupt settings file
/// cannot poison every request header.
pub fn sanitise_install_id(raw: &str) -> String {
    let id = raw.trim();
    if id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        id.to_string()
    } else {
        String::new()
    }
}

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
    settings.upload_server = sanitise_server(&settings.upload_server);
    settings.install_id = sanitise_install_id(&settings.install_id);
}

fn config_file<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    config_path(app, "settings.json")
}

/// A file next to `settings.json` in the app's config directory.
pub(crate) fn config_path<R: Runtime>(app: &AppHandle<R>, name: &str) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(name))
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

/// Writes the file only the user can read: `settings.json` names the save
/// folder and `shares.json` holds the delete token of every shared link,
/// so neither belongs in a world-readable file.
pub(crate) fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("settings path has no directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(&format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = write_private(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// `fs::write`, but the file is created with mode 0600 on unix rather than
/// whatever the umask allows.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    // A file that already existed keeps its old mode, so set it as well.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    file.flush()
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

    /// The checked form says *no* instead of quietly handing back the
    /// default, so the settings window can report a typo (and so
    /// `commands::apply_settings` need not guess from the outcome).
    #[test]
    fn a_server_that_is_not_an_origin_is_rejected_rather_than_replaced() {
        use super::{sanitise_server_checked, DEFAULT_UPLOAD_SERVER};
        assert_eq!(sanitise_server_checked("https://socorin.com").as_deref(), Some(DEFAULT_UPLOAD_SERVER));
        assert_eq!(sanitise_server_checked(" HTTP://Localhost:3000/ ").as_deref(), Some("http://localhost:3000"));
        for bad in ["", "   ", "socorin.com", "ftp://x", "https://socorin.com/api", "https://user:pw@socorin.com", "nope"] {
            assert_eq!(sanitise_server_checked(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_share_server_is_an_http_origin_or_the_default() {
        use super::{sanitise_server, DEFAULT_UPLOAD_SERVER};
        assert_eq!(sanitise_server("https://socorin.com"), DEFAULT_UPLOAD_SERVER);
        assert_eq!(sanitise_server(" https://socorin.com/ "), DEFAULT_UPLOAD_SERVER);
        assert_eq!(sanitise_server("http://localhost:3000"), "http://localhost:3000");
        assert_eq!(sanitise_server("http://localhost:3000///"), "http://localhost:3000");
        assert_eq!(sanitise_server("HTTPS://Share.Example.COM:8443"), "https://share.example.com:8443");
        assert_eq!(sanitise_server("https://example.com:443"), "https://example.com");
        for bad in [
            "",
            "   ",
            "socorin.com",
            "ftp://socorin.com",
            "file:///etc",
            "https://socorin.com/api",
            "https://socorin.com/?x=1",
            "https://socorin.com/#frag",
            "https://user:pw@socorin.com",
            "https://",
            "not a url",
        ] {
            assert_eq!(sanitise_server(bad), DEFAULT_UPLOAD_SERVER, "{bad:?}");
        }
    }

    #[test]
    fn the_install_id_is_kept_only_when_it_looks_like_one() {
        use super::sanitise_install_id;
        assert_eq!(sanitise_install_id("AbC-123_xyz"), "AbC-123_xyz");
        assert_eq!(sanitise_install_id("  AbC  "), "AbC");
        assert_eq!(sanitise_install_id(""), "");
        assert_eq!(sanitise_install_id("has space"), "");
        assert_eq!(sanitise_install_id("new\nline"), "");
        assert_eq!(sanitise_install_id(&"a".repeat(65)), "");
        assert_eq!(sanitise_install_id("ünïcode"), "");
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
    fn update_checks_and_automatic_installs_are_on_by_default() {
        let d = Settings::default();
        assert!(d.check_updates);
        assert!(d.auto_update);
        assert_eq!((d.update_checked_at, d.update_available.as_str(), d.last_version.as_str()), (0, "", ""));
        // A settings file from before the feature existed, or from before
        // automatic installs became the default: same defaults.
        let old: Settings = serde_json::from_str(r#"{"hotkey":"F5"}"#).unwrap();
        assert!(old.check_updates && old.auto_update);
        // The user switched automatic installs off: that is kept.
        let off: Settings = serde_json::from_str(r#"{"autoUpdate":false}"#).unwrap();
        assert!(off.check_updates && !off.auto_update);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["checkUpdates"], true);
        assert_eq!(json["autoUpdate"], true);
        assert_eq!(json["updateCheckedAt"], 0);
        assert_eq!(json["updateAvailable"], "");
        let chosen: Settings = serde_json::from_str(r#"{"checkUpdates":false,"autoUpdate":true,"updateAvailable":" 1.2.3 "}"#).unwrap();
        assert!(!chosen.check_updates && chosen.auto_update);
        let app = app();
        let mut s = chosen;
        normalise(app.handle(), &mut s);
        assert_eq!(s.update_available, "1.2.3");
    }

    /// `settings.json` and `shares.json` (which holds a delete token per
    /// shared link) are the user's alone.
    #[cfg(unix)]
    #[test]
    fn what_is_written_is_readable_by_the_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_support::temp_dir("settings-mode");
        let path = dir.join("shares.json");
        // A file that already exists with wider rights is narrowed as well.
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_atomically(&path, b"[]").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let fresh = dir.join("deeper").join("settings.json");
        super::write_atomically(&fresh, b"{}").unwrap();
        assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o600);
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(dir.join("deeper")).unwrap().count(), 1);
        assert!(super::write_atomically(std::path::Path::new("/"), b"x").is_err());
    }

    #[test]
    fn sharing_defaults_to_socorin_com_and_no_install_id() {
        let d = Settings::default();
        assert_eq!(d.upload_server, DEFAULT_UPLOAD_SERVER);
        assert_eq!(d.install_id, "");
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["uploadServer"], DEFAULT_UPLOAD_SERVER);
        assert_eq!(json["installId"], "");
        assert_eq!(serde_json::to_value(AfterCapture::Upload).unwrap(), "upload");
        // A settings file from before the feature, and one with a broken
        // server or id: normalised to something usable.
        let old: Settings = serde_json::from_str(r#"{"hotkey":"F5"}"#).unwrap();
        assert_eq!(old.upload_server, DEFAULT_UPLOAD_SERVER);
        let mut s: Settings = serde_json::from_str(
            r#"{"uploadServer":"http://localhost:3000/","installId":" abc ","afterCapture":"upload"}"#,
        )
        .unwrap();
        assert_eq!(s.after_capture, AfterCapture::Upload);
        let app = app();
        normalise(app.handle(), &mut s);
        assert_eq!(s.upload_server, "http://localhost:3000");
        assert_eq!(s.install_id, "abc");
        s.upload_server = "nope".into();
        s.install_id = "bad id".into();
        normalise(app.handle(), &mut s);
        assert_eq!(s.upload_server, DEFAULT_UPLOAD_SERVER);
        assert_eq!(s.install_id, "");
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
