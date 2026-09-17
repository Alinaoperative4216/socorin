//! Putting a *file* on the system clipboard, so a recording can be pasted
//! straight into a chat or an e-mail without a trip through the file
//! manager. Each platform has its own way: a file URL on the macOS
//! pasteboard, a `CF_HDROP` list on Windows, `text/uri-list` (plus GNOME's
//! copied-files flavour) through GTK on Linux.

use std::path::Path;

use tauri::{AppHandle, Runtime};

/// Copy `path` (which must exist) to the clipboard as a file reference.
pub fn copy_file<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    platform::copy_file(app, path)
}

/// Runs `f` on the UI thread and waits for its answer (clipboard APIs want
/// the main thread on macOS and Linux).
#[allow(dead_code)]
fn on_main_thread<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "the clipboard did not respond".to_string())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::path::PathBuf;

    pub fn copy_file<R: Runtime>(app: &AppHandle<R>, path: PathBuf) -> Result<(), String> {
        on_main_thread(app, move || crate::macos::copy_file_to_pasteboard(&path))?
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::path::PathBuf;
    use windows_sys::Win32::{
        Foundation::HANDLE,
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_HDROP,
        },
    };

    pub fn copy_file<R: Runtime>(_app: &AppHandle<R>, path: PathBuf) -> Result<(), String> {
        let payload = super::hdrop_payload(&path.to_string_lossy());
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return Err("cannot open the clipboard".into());
            }
            let result = (|| {
                if EmptyClipboard() == 0 {
                    return Err("cannot clear the clipboard".to_string());
                }
                let global = GlobalAlloc(GMEM_MOVEABLE, payload.len());
                if global.is_null() {
                    return Err("out of memory".to_string());
                }
                let dst = GlobalLock(global) as *mut u8;
                if dst.is_null() {
                    return Err("cannot lock clipboard memory".to_string());
                }
                std::ptr::copy_nonoverlapping(payload.as_ptr(), dst, payload.len());
                GlobalUnlock(global);
                if SetClipboardData(CF_HDROP as u32, global as HANDLE).is_null() {
                    return Err("the clipboard refused the file".to_string());
                }
                Ok(())
            })();
            CloseClipboard();
            result
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::path::PathBuf;

    pub fn copy_file<R: Runtime>(app: &AppHandle<R>, path: PathBuf) -> Result<(), String> {
        on_main_thread(app, move || {
            let uri = gtk::glib::filename_to_uri(&path, None)
                .map_err(|e| e.to_string())?
                .to_string();
            let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
            let targets = [
                gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0),
                gtk::TargetEntry::new("x-special/gnome-copied-files", gtk::TargetFlags::empty(), 1),
                gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 2),
            ];
            let ok = clipboard.set_with_data(&targets, move |_, selection, info| match info {
                0 => {
                    selection.set_uris(&[uri.as_str()]);
                }
                1 => {
                    selection.set(&selection.target(), 8, super::gnome_copied_files(&uri).as_bytes());
                }
                _ => {
                    selection.set_text(&uri);
                }
            });
            if ok {
                Ok(())
            } else {
                Err("cannot own the clipboard".to_string())
            }
        })?
    }
}

/// The `CF_HDROP` clipboard payload for one file: a `DROPFILES` header
/// (20 bytes: offset of the list, drop point, non-client flag, wide flag)
/// followed by the UTF-16 path list, each name NUL-terminated and the list
/// ending with an extra NUL.
#[allow(dead_code)]
fn hdrop_payload(path: &str) -> Vec<u8> {
    const HEADER: u32 = 20;
    let mut out = Vec::with_capacity(HEADER as usize + path.len() * 2 + 4);
    out.extend_from_slice(&HEADER.to_le_bytes()); // pFiles
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for unit in path.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0, 0, 0]); // name terminator + list terminator
    out
}

/// GNOME / Nautilus style "copied files" payload (`copy` then the URIs).
#[allow(dead_code)]
fn gnome_copied_files(uri: &str) -> String {
    format!("copy\n{uri}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{app, temp_dir};

    #[test]
    fn only_existing_files_reach_the_clipboard() {
        let app = app();
        let dir = temp_dir("clipboard");
        let err = copy_file(&app, &dir.join("missing.mov")).unwrap_err();
        assert!(err.contains("is not a file"), "{err}");
        // A directory is not a file either (and never touches the real clipboard).
        let err = copy_file(&app, &dir).unwrap_err();
        assert!(err.contains("is not a file"), "{err}");
    }

    #[test]
    fn main_thread_helper_returns_the_closure_result() {
        let app = app();
        assert_eq!(on_main_thread(&app, || 41 + 1).unwrap(), 42);
    }

    #[test]
    fn hdrop_payload_has_the_dropfiles_header_and_a_double_terminated_wide_list() {
        let bytes = hdrop_payload("C:\\a.mp4");
        assert_eq!(&bytes[..4], &20u32.to_le_bytes());
        assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
        let wide: Vec<u16> = bytes[20..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(wide, "C:\\a.mp4\0\0".encode_utf16().collect::<Vec<u16>>());
        assert_eq!(gnome_copied_files("file:///tmp/a.mp4"), "copy\nfile:///tmp/a.mp4");
    }
}
