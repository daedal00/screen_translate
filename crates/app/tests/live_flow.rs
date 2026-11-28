//! Milestone 3b RED: pure helper `screen_translate_app::live::overlay_bounds`.
//! Signature: `pub fn overlay_bounds(target: &Target, rect: LogicalRect) -> (f64, f64, f64, f64)`
//! returning (x, y, w, h) of the overlay window in the platform's global units:
//! - macOS: logical points, x = target.pos.0 / scale + rect.x, y = target.pos.1 / scale + rect.y, w/h = rect w/h.
//! - Windows: physical px, x = pos.0 + round(rect.x * scale), y likewise, w/h = round(rect.w/h * scale).
use screen_translate_app::live::overlay_bounds;
use screen_translate_core::{capture::LogicalRect, pipeline::Target};

#[cfg(target_os = "macos")]
#[test]
fn macos_logical_points() {
    let t = Target {
        scale: 2.0,
        pos: (2880, 0),
        size: (2560, 1440),
        origin: (1440, 0),
        display_id: 1,
    };
    let r = LogicalRect {
        x: 100.5,
        y: 40.0,
        width: 300.0,
        height: 80.25,
    };
    assert_eq!(overlay_bounds(&t, r), (1440.0 + 100.5, 40.0, 300.0, 80.25));
}

#[cfg(windows)]
#[test]
fn windows_physical_px() {
    let t = Target {
        scale: 1.5,
        pos: (1920, -100),
        size: (2880, 1620),
        origin: (1920, -100),
        display_id: 1,
    };
    let r = LogicalRect {
        x: 10.0,
        y: 20.0,
        width: 301.0,
        height: 81.0,
    };
    assert_eq!(overlay_bounds(&t, r), (1935.0, -70.0, 452.0, 122.0));
}
