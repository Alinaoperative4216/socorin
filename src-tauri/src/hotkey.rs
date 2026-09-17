//! Global shortcut registration.

use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::{capture, record, settings::Settings};

/// (Re)register every global shortcut from the given settings.
pub fn apply<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), String> {
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;

    let region = settings.hotkey.trim();
    if !region.is_empty() {
        gs.on_shortcut(region, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                capture::begin_region(app.clone());
            }
        })
        .map_err(|e| format!("cannot register \"{region}\": {e}"))?;
    }

    let full = settings.fullscreen_hotkey.trim();
    if !full.is_empty() && !full.eq_ignore_ascii_case(region) {
        gs.on_shortcut(full, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                capture::begin_fullscreen(app.clone());
            }
        })
        .map_err(|e| format!("cannot register \"{full}\": {e}"))?;
    }

    let record_key = settings.record_hotkey.trim();
    if !record_key.is_empty()
        && !record_key.eq_ignore_ascii_case(region)
        && !record_key.eq_ignore_ascii_case(full)
    {
        gs.on_shortcut(record_key, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                // Starts a region recording, or stops the running one.
                record::begin_region(app.clone());
            }
        })
        .map_err(|e| format!("cannot register \"{record_key}\": {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::app;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Unusual combinations so the tests never clash with the running app.
    const REGION: &str = "Ctrl+Alt+Shift+F18";
    const FULL: &str = "Ctrl+Alt+Shift+F19";
    const RECORD: &str = "Ctrl+Alt+Shift+F20";

    fn settings(hotkey: &str, full: &str, record: &str) -> Settings {
        Settings {
            hotkey: hotkey.into(),
            fullscreen_hotkey: full.into(),
            record_hotkey: record.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn registers_every_distinct_shortcut_and_replaces_them_on_the_next_apply() {
        let app = app();
        let handle = app.handle();
        let gs = handle.global_shortcut();

        apply(handle, &settings(REGION, FULL, RECORD)).unwrap();
        assert!(gs.is_registered(REGION));
        assert!(gs.is_registered(FULL));
        assert!(gs.is_registered(RECORD));

        // Duplicates of the region shortcut are skipped rather than failing.
        apply(handle, &settings(REGION, &REGION.to_lowercase(), REGION)).unwrap();
        assert!(gs.is_registered(REGION));
        assert!(!gs.is_registered(FULL));

        apply(handle, &settings("", "", "")).unwrap();
        assert!(!gs.is_registered(REGION));
    }

    #[test]
    fn reports_which_shortcut_could_not_be_registered() {
        let app = app();
        let handle = app.handle();
        let err = apply(handle, &settings("Bogus+", "", "")).unwrap_err();
        assert!(err.contains("cannot register \"Bogus+\""), "{err}");
        let err = apply(handle, &settings(REGION, "NotAKey", "")).unwrap_err();
        assert!(err.contains("NotAKey"), "{err}");
        let err = apply(handle, &settings(REGION, FULL, "Nope")).unwrap_err();
        assert!(err.contains("Nope"), "{err}");
        apply(handle, &settings("", "", "")).unwrap();
    }
}
