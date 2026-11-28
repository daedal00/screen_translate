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
        let (a, b) = (
            start.min(start + len) * scale,
            start.max(start + len) * scale,
        );
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        let lo = a.floor().clamp(0.0, max as f64) as u32;
        let hi = b.ceil().clamp(0.0, max as f64) as u32;
        (hi > lo).then_some((lo, hi - lo))
    };
    let (x, width) = span(rect.x, rect.width, img_w)?;
    let (y, height) = span(rect.y, rect.height, img_h)?;
    Some(PhysicalRect {
        x,
        y,
        width,
        height,
    })
}

pub fn crop(img: &RgbaImage, rect: PhysicalRect) -> RgbaImage {
    image::imageops::crop_imm(img, rect.x, rect.y, rect.width, rect.height).to_image()
}

/// Index of the origin exactly equal to `target`. Both must be in the same unit system.
pub fn pick_monitor(origins: &[(i32, i32)], target: (i32, i32)) -> Option<usize> {
    origins.iter().position(|&o| o == target)
}

/// Capture `rect` (monitor-local logical px) of the monitor whose origin is `target`, in xcap's own
/// units: physical pixels on Windows (`scale` converts), points on macOS (scale is 1). Falls back
/// to the primary monitor. `Ok(None)` when the rect maps to no pixels.
pub fn grab_region(
    target: (i32, i32),
    rect: LogicalRect,
    scale: f64,
) -> Result<Option<RgbaImage>, String> {
    RegionGrabber::new(target, scale).grab(rect)
}

/// Repeated `grab_region` calls for one live session. On Windows the monitor is looked up once and
/// reused until a capture fails (UNVERIFIED on Windows); macOS looks it up on every grab.
pub struct RegionGrabber {
    target: (i32, i32),
    scale: f64,
    monitor: Option<Monitor>,
}

impl RegionGrabber {
    pub fn new(target: (i32, i32), scale: f64) -> Self {
        Self {
            target,
            scale,
            monitor: None,
        }
    }

    fn find_monitor(&self) -> Result<Monitor, String> {
        let monitors = Monitor::all().map_err(|e| format!("cannot list monitors: {e}"))?;
        let origins: Vec<(i32, i32)> = monitors
            .iter()
            .map(|m| (m.x().unwrap_or(i32::MIN), m.y().unwrap_or(i32::MIN)))
            .collect();
        let i = pick_monitor(&origins, self.target)
            .or_else(|| {
                monitors
                    .iter()
                    .position(|m| m.is_primary().unwrap_or(false))
            })
            .ok_or("no monitor found")?;
        Ok(monitors.into_iter().nth(i).expect("index from this list"))
    }

    pub fn grab(&mut self, rect: LogicalRect) -> Result<Option<RgbaImage>, String> {
        let monitor = match self.monitor.take() {
            Some(m) => m,
            None => self.find_monitor()?,
        };
        let (w, h) = (
            monitor.width().map_err(|e| e.to_string())?,
            monitor.height().map_err(|e| e.to_string())?,
        );
        let scale = if cfg!(target_os = "macos") {
            1.0
        } else {
            self.scale
        };
        let Some(r) = to_physical(rect, scale, w, h) else {
            self.keep(monitor);
            return Ok(None);
        };
        let image = monitor.capture_region(r.x, r.y, r.width, r.height);
        // A failed capture drops the monitor, so the next grab looks it up again (display changes).
        if image.is_ok() {
            self.keep(monitor);
        }
        image.map(Some).map_err(|e| {
            format!("screen capture failed (on macOS grant Screen Recording permission): {e}")
        })
    }

    fn keep(&mut self, monitor: Monitor) {
        if cfg!(windows) {
            self.monitor = Some(monitor);
        }
    }
}
