//! Opt-in diagnostics, all controlled by environment variables so they never
//! affect a normal run:
//!
//! - `SOCORIN_DEBUG=1`                  verbose logging on stderr
//! - `SOCORIN_DEBUG_DIR=/path`          dump captured / cropped PNGs there
//! - `SOCORIN_DEBUG_AUTOSELECT=x,y,w,h` overlay auto-selects this region (CSS px)
//! - `SOCORIN_DEBUG_AUTOACTION=save|copy` the (in-place) editor adds sample annotations and runs the action
//! - `SOCORIN_DEBUG_UPDATE_URL=http://…`  read the update manifest from there (see `update::version_url`)
//!
//! Together they allow an end-to-end run without touching mouse or keyboard.
//!
//! Only the logging exists in a release build. The rest (the "harness") is
//! compiled in for debug builds and for release builds made with
//! `--features debug-harness`: a shipped binary must not let whoever sets
//! its environment dump the screen to disk or take screenshots without the
//! user, since the app holds the Screen Recording permission.

use std::{fs, path::PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugOptions {
    pub enabled: bool,
    pub dump_dir: Option<String>,
    pub auto_select: Option<[f64; 4]>,
    pub auto_action: Option<String>,
}

/// Whether the environment may steer captures (debug builds, or release
/// builds made with `--features debug-harness`).
pub const HARNESS: bool = cfg!(any(debug_assertions, feature = "debug-harness"));

pub fn options() -> DebugOptions {
    if !HARNESS {
        return DebugOptions {
            enabled: std::env::var_os("SOCORIN_DEBUG").is_some(),
            ..DebugOptions::default()
        };
    }
    let dump_dir = std::env::var("SOCORIN_DEBUG_DIR").ok().filter(|s| !s.is_empty());
    let auto_select = std::env::var("SOCORIN_DEBUG_AUTOSELECT").ok().and_then(|s| {
        let v: Vec<f64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
    });
    let auto_action = std::env::var("SOCORIN_DEBUG_AUTOACTION").ok().filter(|s| !s.is_empty());
    DebugOptions {
        enabled: std::env::var_os("SOCORIN_DEBUG").is_some()
            || dump_dir.is_some()
            || auto_select.is_some()
            || auto_action.is_some(),
        dump_dir,
        auto_select,
        auto_action,
    }
}

pub fn log(msg: impl AsRef<str>) {
    if options().enabled {
        eprintln!("[socorin] {}", msg.as_ref());
    }
}

pub fn dump(name: &str, bytes: &[u8]) {
    if let Some(dir) = options().dump_dir {
        let dir = PathBuf::from(dir);
        if fs::create_dir_all(&dir).is_ok() {
            let path = dir.join(name);
            match fs::write(&path, bytes) {
                Ok(()) => log(format!("dumped {}", path.display())),
                Err(e) => eprintln!("[socorin] cannot dump {}: {e}", path.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The environment is process-wide: one test at a time.
    static ENV: Mutex<()> = Mutex::new(());
    const VARS: [&str; 4] = [
        "SOCORIN_DEBUG",
        "SOCORIN_DEBUG_DIR",
        "SOCORIN_DEBUG_AUTOSELECT",
        "SOCORIN_DEBUG_AUTOACTION",
    ];

    fn with_env<T>(vars: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        for v in VARS {
            std::env::remove_var(v);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        let out = f();
        for v in VARS {
            std::env::remove_var(v);
        }
        out
    }

    #[test]
    fn is_off_by_default() {
        with_env(&[], || {
            let o = options();
            assert!(!o.enabled);
            assert!(o.dump_dir.is_none() && o.auto_select.is_none() && o.auto_action.is_none());
            log("silent");
            dump("nothing.png", b"x");
        });
    }

    #[test]
    fn any_variable_switches_the_diagnostics_on() {
        with_env(&[("SOCORIN_DEBUG", "1")], || assert!(options().enabled));
        with_env(&[("SOCORIN_DEBUG_AUTOACTION", "save")], || {
            let o = options();
            assert!(o.enabled);
            assert_eq!(o.auto_action.as_deref(), Some("save"));
        });
        with_env(&[("SOCORIN_DEBUG_AUTOACTION", ""), ("SOCORIN_DEBUG_DIR", "")], || {
            assert!(!options().enabled);
        });
    }

    #[test]
    fn parses_the_auto_select_rectangle() {
        with_env(&[("SOCORIN_DEBUG_AUTOSELECT", " 10, 20 ,300,400")], || {
            assert_eq!(options().auto_select, Some([10.0, 20.0, 300.0, 400.0]));
        });
        with_env(&[("SOCORIN_DEBUG_AUTOSELECT", "1,2,3")], || assert_eq!(options().auto_select, None));
        with_env(&[("SOCORIN_DEBUG_AUTOSELECT", "a,b,c,d")], || assert_eq!(options().auto_select, None));
    }

    #[test]
    fn dumps_into_the_requested_directory() {
        let dir = std::env::temp_dir().join(format!("screenshot-app-dump-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        with_env(&[("SOCORIN_DEBUG_DIR", dir.to_str().unwrap())], || {
            assert!(options().enabled);
            dump("crop.png", b"png");
            assert_eq!(fs::read(dir.join("crop.png")).unwrap(), b"png");
            // A name that cannot be written is reported, not fatal.
            dump("missing/sub/crop.png", b"png");
        });
        let _ = fs::remove_dir_all(&dir);
    }
}
