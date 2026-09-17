mod capture;
mod clipboard;
mod commands;
mod cursor;
mod debug;
mod hotkey;
#[cfg(target_os = "macos")]
mod macos;
mod record;
mod settings;
mod tray;
mod update;
mod windows;
#[cfg(test)]
mod test_support;

use std::sync::Mutex;

use tauri::{Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        // Must be the first plugin so a second launch is intercepted early.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            handle_cli_args(app, &args, true);
        }));
    configure(builder)
        .setup(|app| {
            let handle = app.handle().clone();
            let settings = settings::load(&handle);
            app.manage(settings::SettingsState(Mutex::new(settings.clone())));

            // Menu-bar / tray app: no Dock icon on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::create(&handle)?;

            // First launch: say that the app lives in the menu bar / tray.
            if !settings.welcome_shown {
                if let Err(e) = windows::open_welcome(&handle) {
                    eprintln!("[welcome] {e}");
                }
            }

            if let Err(e) = hotkey::apply(&handle, &settings) {
                eprintln!("[hotkey] {e}");
            }

            sync_autostart(&handle, &settings);

            // A version announced earlier goes back into the tray menu; an
            // update that was installed is mentioned once; the daily check
            // is scheduled.
            update::on_startup(&handle);
            update::start(&handle);

            // Overlay webviews are expensive to create, so build them now,
            // hidden, instead of on the first hotkey press.
            let prewarm = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                windows::prewarm_overlays(&prewarm);
                windows::prewarm_recorder(&prewarm);
            });

            let args: Vec<String> = std::env::args().collect();
            handle_cli_args(&handle, &args, false);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Closing the last window must not quit a tray app.
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

/// Everything the app needs besides its setup and its event loop: the
/// plugins, the shared state, the window-close policy and the commands. The
/// unit tests build the same thing on Tauri's mock runtime.
pub(crate) fn configure<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(capture::AppState::default())
        .manage(record::RecordState::default())
        .manage(update::UpdateState::default())
        .on_window_event(|window, event| match event {
            // The settings window is hidden, not destroyed, so the app keeps
            // living in the tray.
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == windows::MAIN {
                    api.prevent_close();
                    let _ = window.hide();
                } else if window.label() == windows::WELCOME {
                    // Closed by other means than OK (window manager, Cmd+W):
                    // still do not nag again.
                    let _ = settings::mark_welcome_shown(window.app_handle());
                }
            }
            // The update popover goes away when the user clicks elsewhere.
            tauri::WindowEvent::Focused(false) if window.label() == windows::UPDATE => {
                update::blurred(window.app_handle());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::platform_info,
            commands::request_screen_permission,
            commands::start_capture,
            commands::start_fullscreen_capture,
            commands::cancel_capture,
            commands::overlay_info,
            commands::overlay_pixels,
            commands::overlay_ready,
            commands::finish_region,
            commands::begin_annotation,
            commands::edit_png,
            commands::save_png_as,
            commands::pending_image,
            commands::save_png,
            commands::copy_png,
            commands::get_settings,
            commands::update_settings,
            commands::welcome_ready,
            commands::dismiss_welcome,
            commands::debug_options,
            commands::debug_log,
            commands::default_save_dir,
            commands::open_save_dir,
            commands::open_screen_permission_settings,
            commands::start_record_region,
            commands::start_record_fullscreen,
            commands::start_recording,
            commands::stop_recording,
            commands::stop_recording_copy,
            commands::cancel_recording,
            commands::recording_status,
            commands::show_settings,
            commands::update_status,
            commands::check_for_updates,
            commands::install_update,
            commands::dismiss_update,
            commands::update_ready,
            commands::quit,
        ])
}

/// `--capture` / `--capture-full` trigger a capture, `--record` /
/// `--record-full` / `--stop-record` a recording (handy for binding a
/// desktop-environment shortcut on Wayland, where global hotkeys are not
/// available); `--minimized` (autostart) stays in the tray; anything else on
/// a second launch shows the settings window.
///
/// The capture / record triggers only work once the user has enabled them
/// in Settings: this app holds the Screen Recording permission, and without
/// the switch any local program could launch it with `--capture-full` to
/// get a screenshot it is not allowed to take itself.
fn handle_cli_args<R: tauri::Runtime>(app: &tauri::AppHandle<R>, args: &[String], second_instance: bool) {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let wants_trigger = TRIGGER_FLAGS.iter().any(|f| has(f));
    if wants_trigger && !settings::current(app).cli_triggers {
        let shown: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
        eprintln!("[cli] {} ignored: enable \"Allow command-line triggers\" in Settings", shown.join(" "));
        windows::show_settings(app);
        let _ = app.emit(
            "capture-error",
            "Command-line triggers are off. Turn on \"Allow command-line triggers\" under System to use them.",
        );
        return;
    }
    if has("--capture") {
        capture::begin_region(app.clone());
    } else if has("--capture-full") {
        capture::begin_fullscreen(app.clone());
    } else if has("--cancel") {
        capture::cancel(app);
    } else if has("--record") {
        record::begin_region(app.clone());
    } else if has("--record-full") {
        record::begin_fullscreen(app.clone());
    } else if has("--stop-record") {
        record::stop_async(app.clone());
    } else if second_instance && !has("--minimized") {
        windows::show_settings(app);
    }
}

/// Arguments that start a capture or a recording.
const TRIGGER_FLAGS: [&str; 4] = ["--capture", "--capture-full", "--record", "--record-full"];

/// Registers the app as a login item when the setting (on by default) says
/// so. `enable` rewrites the entry every time, so an app that moved (dragged
/// from the disk image into Applications) is registered under its new path
/// on the next launch. A copy running from the disk image or from
/// Gatekeeper's translocation folder is skipped: that path is gone once the
/// app is installed and would only start a stale copy at login. Switching
/// the setting off is handled where it is saved (`commands::update_settings`),
/// so nothing is removed here.
fn sync_autostart<R: tauri::Runtime>(app: &tauri::AppHandle<R>, settings: &settings::Settings) {
    if !settings.autostart {
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(issue) = macos::install_issue() {
        eprintln!("[autostart] not registering a {issue} copy as a login item");
        return;
    }
    if let Err(e) = app.autolaunch().enable() {
        eprintln!("[autostart] {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{app_with, settings_in, temp_dir};
    use crate::settings::Settings;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("socorin".to_string())
            .chain(list.iter().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn capture_triggers_are_ignored_until_the_user_allows_them() {
        let app = app_with(settings_in(&temp_dir("cli-off")));
        for flag in TRIGGER_FLAGS {
            handle_cli_args(&app, &args(&[flag]), true);
            assert!(!capture::is_busy(&app));
        }
    }

    #[test]
    fn allowed_triggers_reach_the_capture_and_record_entry_points() {
        let dir = temp_dir("cli-triggers");
        let app = app_with(Settings { cli_triggers: true, ..settings_in(&dir) });
        // A session is running, so the entry points return right away
        // instead of grabbing the screen.
        let session = test_support::start_session(&app, vec![test_support::shot(test_support::geom(1, 0, 0, 8, 8, 1.0, true))]);
        handle_cli_args(&app, &args(&["--capture"]), false);
        handle_cli_args(&app, &args(&["--capture-full"]), false);
        handle_cli_args(&app, &args(&["--record"]), false);
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(app.state::<capture::AppState>().session.load(std::sync::atomic::Ordering::SeqCst), session);
        assert!(capture::is_busy(&app));
    }

    #[test]
    fn housekeeping_flags_work_without_the_switch() {
        let dir = temp_dir("cli-on");
        let app = app_with(Settings { cli_triggers: true, ..settings_in(&dir) });
        handle_cli_args(&app, &args(&["--cancel"]), false);
        assert!(!capture::is_busy(&app));
        handle_cli_args(&app, &args(&["--stop-record"]), false);
        std::thread::sleep(std::time::Duration::from_millis(30));
        handle_cli_args(&app, &args(&["--minimized"]), true);
        handle_cli_args(&app, &args(&[]), true); // second launch: settings window
        handle_cli_args(&app, &args(&[]), false); // first launch: nothing
        assert!(!record::is_recording(&app));
    }

    #[test]
    fn the_login_item_follows_the_setting_at_startup() {
        let dir = temp_dir("autostart-sync");
        let app = app_with(settings_in(&dir));
        let _ = app.autolaunch().disable();
        assert_eq!(app.autolaunch().is_enabled().unwrap(), false);

        // Off: left alone (removing it is done when the setting is saved).
        sync_autostart(&app, &Settings { autostart: false, ..Settings::default() });
        assert_eq!(app.autolaunch().is_enabled().unwrap(), false);

        // On (the default): registered, and registering again is harmless.
        sync_autostart(&app, &Settings::default());
        assert!(app.autolaunch().is_enabled().unwrap());
        sync_autostart(&app, &Settings::default());
        assert!(app.autolaunch().is_enabled().unwrap());
        let _ = app.autolaunch().disable();
    }
}
