//! Where is the mouse, expressed in an overlay window's own logical
//! coordinates. Used while a capture session is active so the crosshair and
//! keyboard focus follow the cursor without relying on the webview receiving
//! mouse-move events (macOS only delivers those to the key window).

use tauri::{AppHandle, Runtime};

use crate::capture::MonitorGeom;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorHit {
    pub monitor_id: u32,
    /// Logical position inside that monitor / overlay window (CSS px).
    pub x: f64,
    pub y: f64,
}

fn hit(geoms: &[MonitorGeom], gx: f64, gy: f64) -> Option<CursorHit> {
    geoms
        .iter()
        .find(|g| {
            gx >= g.x as f64
                && gx < (g.x + g.width as i32) as f64
                && gy >= g.y as f64
                && gy < (g.y + g.height as i32) as f64
        })
        .map(|g| CursorHit {
            monitor_id: g.id,
            x: gx - g.x as f64,
            y: gy - g.y as f64,
        })
}

#[cfg(target_os = "macos")]
pub fn locate<R: Runtime>(_app: &AppHandle<R>, geoms: &[MonitorGeom]) -> Option<CursorHit> {
    let (gx, gy) = crate::macos::cursor_logical()?;
    hit(geoms, gx, gy)
}

#[cfg(target_os = "windows")]
pub fn locate<R: Runtime>(app: &AppHandle<R>, geoms: &[MonitorGeom]) -> Option<CursorHit> {
    // Physical virtual-screen coordinates; geometry was normalised to logical
    // per monitor, so compare in each monitor's own physical space.
    let p = app.cursor_position().ok()?;
    geoms.iter().find_map(|g| {
        let s = g.scale as f64;
        let (x0, y0) = (g.x as f64 * s, g.y as f64 * s);
        let (w, h) = (g.width as f64 * s, g.height as f64 * s);
        (p.x >= x0 && p.x < x0 + w && p.y >= y0 && p.y < y0 + h).then(|| CursorHit {
            monitor_id: g.id,
            x: (p.x - x0) / s,
            y: (p.y - y0) / s,
        })
    })
}

#[cfg(target_os = "linux")]
pub fn locate<R: Runtime>(app: &AppHandle<R>, geoms: &[MonitorGeom]) -> Option<CursorHit> {
    let p = app.cursor_position().ok()?;
    let s = geoms.first().map(|g| g.scale as f64).unwrap_or(1.0).max(0.1);
    hit(geoms, p.x / s, p.y / s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{app, geom};

    #[test]
    fn hit_maps_a_global_point_into_the_monitor_under_it() {
        let geoms = [geom(1, 0, 0, 1000, 500, 2.0, true), geom(2, 1000, -100, 800, 600, 1.0, false)];
        assert_eq!(hit(&geoms, 10.0, 20.0), Some(CursorHit { monitor_id: 1, x: 10.0, y: 20.0 }));
        assert_eq!(hit(&geoms, 1000.0, 0.0), Some(CursorHit { monitor_id: 2, x: 0.0, y: 100.0 }));
        assert_eq!(hit(&geoms, 999.9, 499.9).map(|h| h.monitor_id), Some(1));
        assert_eq!(hit(&geoms, 1800.0, 0.0), None);
        assert_eq!(hit(&geoms, -1.0, 0.0), None);
        assert_eq!(hit(&[], 0.0, 0.0), None);
    }

    #[test]
    fn locate_uses_the_native_cursor_position() {
        let app = app();
        // A monitor large enough to contain any real cursor position.
        let everywhere = [geom(7, -100_000, -100_000, 200_000, 200_000, 1.0, true)];
        let hit = locate(app.handle(), &everywhere);
        if let Some(h) = hit {
            assert_eq!(h.monitor_id, 7);
            assert!(h.x >= 0.0 && h.y >= 0.0);
        }
        assert_eq!(locate(app.handle(), &[]), None);
    }
}
