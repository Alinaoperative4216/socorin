//! macOS-only helpers: screen-recording permission, window levels and the
//! file pasteboard.

use std::{ffi::CString, path::Path};

use objc2::{class, msg_send, runtime::AnyObject, Encode, Encoding};
use tauri::{AppHandle, Runtime};
#[cfg(not(test))]
use tauri::WebviewWindow;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

/// Mouse position in global logical points with a top-left origin (the same
/// space xcap reports monitor bounds in).
pub fn cursor_logical() -> Option<(f64, f64)> {
    let p: CGPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
    let main = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    if !p.x.is_finite() || !p.y.is_finite() {
        return None;
    }
    Some((p.x, main.size.height - p.y))
}

/// Bring the app to the front. Since macOS 14 `activateIgnoringOtherApps:`
/// is only a request, so ask through both APIs.
pub fn activate_app<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.run_on_main_thread(|| unsafe {
        let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![ns_app, activateIgnoringOtherApps: true];
        let running: *mut AnyObject = msg_send![class!(NSRunningApplication), currentApplication];
        // NSApplicationActivateIgnoringOtherApps
        let _: bool = msg_send![running, activateWithOptions: 2usize];
    });
}

/// Why this copy of the app cannot be granted (or keep) permissions:
/// `"disk-image"` when it runs from a mounted image / external volume,
/// `"translocated"` when Gatekeeper runs a temporary relocated copy. In both
/// cases macOS usually does not even show the Screen Recording prompt and
/// the app never appears in System Settings; it has to live in Applications.
pub fn install_issue() -> Option<&'static str> {
    install_issue_for(&std::env::current_exe().ok()?.to_string_lossy())
}

fn install_issue_for(exe_path: &str) -> Option<&'static str> {
    if exe_path.contains("/AppTranslocation/") {
        Some("translocated")
    } else if exe_path.starts_with("/Volumes/") {
        Some("disk-image")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::install_issue_for;

    #[test]
    fn detects_where_permissions_cannot_stick() {
        assert_eq!(install_issue_for("/Applications/Socorin.app/Contents/MacOS/socorin"), None);
        assert_eq!(install_issue_for("/Users/me/dev/target/release/socorin"), None);
        assert_eq!(install_issue_for("/Volumes/Socorin/Socorin.app/Contents/MacOS/socorin"), Some("disk-image"));
        assert_eq!(
            install_issue_for("/private/var/folders/xx/T/AppTranslocation/1F2E/d/Socorin.app/Contents/MacOS/socorin"),
            Some("translocated")
        );
    }
}

pub fn install_issue_message(issue: &str) -> String {
    let where_ = if issue == "translocated" {
        "a temporary copy made by Gatekeeper".to_string()
    } else {
        "the disk image (or an external volume)".to_string()
    };
    format!(
        "Socorin is running from {where_}, so macOS cannot grant it the Screen Recording permission. \
         Quit, drag Socorin.app into the Applications folder, eject the disk image and start it from there."
    )
}

pub fn has_screen_permission() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Shows the system prompt (first time only) and returns whether access is granted.
pub fn request_screen_permission() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

/// NSScreenSaverWindowLevel: above the menu bar, the Dock and fullscreen apps.
pub const OVERLAY_LEVEL: isize = 1000;
/// `screencapture -v` dims everything but the recorded area with windows at
/// level 1499; the recording bar sits just above them so it stays readable
/// and clickable.
pub const RECORDER_LEVEL: isize = 1500;
/// CanJoinAllSpaces | Stationary | IgnoresCycle | FullScreenAuxiliary
#[cfg(not(test))]
const OVERLAY_COLLECTION: usize = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);

/// Raise the overlay above everything (menu bar included) on every Space.
/// Not compiled for the tests: their mock windows have no NSWindow.
#[cfg(not(test))]
pub fn raise_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    raise_window(window, OVERLAY_LEVEL);
}

/// Put a borderless window at `level`, on every Space, without a shadow.
#[cfg(not(test))]
pub fn raise_window<R: Runtime>(window: &WebviewWindow<R>, level: isize) {
    let w = window.clone();
    let _ = window.run_on_main_thread(move || {
        if let Ok(ptr) = w.ns_window() {
            let ns: *mut AnyObject = ptr.cast();
            unsafe {
                let _: () = msg_send![ns, setLevel: level];
                let _: () = msg_send![ns, setCollectionBehavior: OVERLAY_COLLECTION];
                let _: () = msg_send![ns, setHasShadow: false];
            }
        }
    });
}

/// Put `path` on the general pasteboard as a file URL: what Finder, Mail
/// and chat apps paste as an attachment. Main thread only.
pub fn copy_file_to_pasteboard(path: &Path) -> Result<(), String> {
    let c_path = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    unsafe {
        let ns_path: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        if ns_path.is_null() {
            return Err("cannot convert the path".into());
        }
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_path];
        if url.is_null() {
            return Err("cannot make a file URL".into());
        }
        let items: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: url];
        let pasteboard: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        let _: isize = msg_send![pasteboard, clearContents];
        let written: bool = msg_send![pasteboard, writeObjects: items];
        if written {
            Ok(())
        } else {
            Err("the pasteboard refused the file".into())
        }
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn a_normal_install_has_no_issue_and_the_messages_name_the_place() {
        assert_eq!(install_issue(), None); // the test binary lives in target/
        let msg = install_issue_message("translocated");
        assert!(msg.contains("Gatekeeper"));
        let msg = install_issue_message("disk-image");
        assert!(msg.contains("disk image"));
    }

    #[test]
    fn a_path_with_a_nul_byte_never_reaches_the_pasteboard() {
        let err = copy_file_to_pasteboard(Path::new("/tmp/a\0b.mov")).unwrap_err();
        assert!(err.contains("nul"), "{err}");
    }

    #[test]
    fn window_levels_are_ordered_over_the_system_dimming() {
        assert!(RECORDER_LEVEL > OVERLAY_LEVEL);
        assert!(RECORDER_LEVEL > 1499);
    }

    #[test]
    fn permission_preflight_and_cursor_do_not_need_the_permission() {
        // Whatever the answer, asking must not prompt or panic.
        let _ = has_screen_permission();
        if let Some((x, y)) = cursor_logical() {
            assert!(x.is_finite() && y.is_finite());
        }
    }
}
