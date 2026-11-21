use image::RgbaImage;
use xcap::Monitor;

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Scale, round outward (floor x/y, ceil right/bottom), clamp to image, normalize negative w/h, None if empty.
pub fn to_physical(rect: LogicalRect, scale: f64, img_w: u32, img_h: u32) -> Option<PhysicalRect> {
    let span = |start: f64, len: f64, max: u32| -> Option<(u32, u32)> {
        let (a, b) = (start.min(start + len) * scale, start.max(start + len) * scale);
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        let lo = a.floor().clamp(0.0, max as f64) as u32;
        let hi = b.ceil().clamp(0.0, max as f64) as u32;
        (hi > lo).then_some((lo, hi - lo))
    };
    let (x, width) = span(rect.x, rect.width, img_w)?;
    let (y, height) = span(rect.y, rect.height, img_h)?;
    Some(PhysicalRect { x, y, width, height })
}

pub fn crop(img: &RgbaImage, rect: PhysicalRect) -> RgbaImage {
    image::imageops::crop_imm(img, rect.x, rect.y, rect.width, rect.height).to_image()
}

/// Index of the origin exactly equal to `target`. Both must be in the same unit system.
pub fn pick_monitor(origins: &[(i32, i32)], target: (i32, i32)) -> Option<usize> {
    origins.iter().position(|&o| o == target)
}

/// Capture the monitor whose origin is `target`, in xcap's own units: physical pixels on
/// Windows, points on macOS. Falls back to the primary monitor.
pub fn grab(target: (i32, i32)) -> Result<RgbaImage, String> {
    let monitors = Monitor::all().map_err(|e| format!("cannot list monitors: {e}"))?;
    let origins: Vec<(i32, i32)> = monitors
        .iter()
        .map(|m| (m.x().unwrap_or(i32::MIN), m.y().unwrap_or(i32::MIN)))
        .collect();
    let monitor = pick_monitor(&origins, target)
        .map(|i| &monitors[i])
        .or_else(|| monitors.iter().find(|m| m.is_primary().unwrap_or(false)))
        .ok_or("no monitor found")?;
    monitor.capture_image().map_err(|e| {
        format!("screen capture failed (on macOS grant Screen Recording permission): {e}")
    })
}
