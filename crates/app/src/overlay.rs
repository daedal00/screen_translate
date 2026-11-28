//! Window bounds for the selector and popup on a monitor. Pure, so it is unit-testable.

use screen_translate_core::capture::LogicalRect;
use screen_translate_core::pipeline::Target;
use screen_translate_core::popup::place;

use crate::popup::POPUP_SIZE;

const POPUP_MARGIN: f64 = 8.0;

/// Whether gpui's `WindowOptions::window_bounds` are virtual-desktop coordinates (Windows:
/// `retrieve_window_placement` scales them by the target display's scale and `check_given_bounds` checks
/// the physical center against that monitor) or display-local ones (macOS: `window.rs` adds the target
/// screen's frame origin).
pub const GLOBAL_BOUNDS: bool = cfg!(windows);

/// `local` (monitor-local logical px) as gpui window bounds for `target`'s display.
pub fn display_bounds(target: &Target, local: LogicalRect, global: bool) -> LogicalRect {
    if !global {
        return local;
    }
    LogicalRect {
        x: local.x + target.pos.0 as f64 / target.scale,
        y: local.y + target.pos.1 as f64 / target.scale,
        ..local
    }
}

fn screen(target: &Target) -> LogicalRect {
    LogicalRect {
        x: 0.0,
        y: 0.0,
        width: target.size.0 as f64 / target.scale,
        height: target.size.1 as f64 / target.scale,
    }
}

/// The selector covers the whole monitor.
pub fn selector_bounds(target: &Target, global: bool) -> LogicalRect {
    display_bounds(target, screen(target), global)
}

/// The popup next to the selection `rect` (monitor-local logical px).
pub fn popup_bounds(target: &Target, rect: LogicalRect, global: bool) -> LogicalRect {
    let (x, y) = place(
        rect,
        POPUP_SIZE.0,
        POPUP_SIZE.1,
        screen(target),
        POPUP_MARGIN,
    );
    display_bounds(
        target,
        LogicalRect {
            x,
            y,
            width: POPUP_SIZE.0,
            height: POPUP_SIZE.1,
        },
        global,
    )
}
