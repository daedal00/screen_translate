//! Pure core of live mode: change detection and overlay layout.

use image::RgbaImage;

use crate::capture::PhysicalRect;

const GRID: u32 = 32;
/// Cells whose mean luma differs by more than this count as changed.
const CELL_DELTA: f32 = 16.0;

/// Mean luma over a fixed 32x32 grid of cells covering the whole image.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint(Vec<f32>);

pub fn fingerprint(img: &RgbaImage) -> Fingerprint {
    let (w, h) = img.dimensions();
    let mut cells = vec![0f32; (GRID * GRID) as usize];
    if w == 0 || h == 0 {
        return Fingerprint(cells);
    }
    // Cells never come out empty, so images smaller than the grid repeat pixels.
    let span = |i: u32, n: u32| {
        let start = i * n / GRID;
        (start, ((i + 1) * n / GRID).max(start + 1).min(n))
    };
    for cy in 0..GRID {
        let (y0, y1) = span(cy, h);
        for cx in 0..GRID {
            let (x0, x1) = span(cx, w);
            let mut sum = 0f32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let [r, g, b, _] = img.get_pixel(x, y).0;
                    sum += 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                }
            }
            cells[(cy * GRID + cx) as usize] = sum / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    Fingerprint(cells)
}

/// Number of cells whose mean luma differs by more than `CELL_DELTA`.
pub fn distance(a: &Fingerprint, b: &Fingerprint) -> u32 {
    a.0.iter().zip(&b.0).filter(|(x, y)| (**x - **y).abs() > CELL_DELTA).count() as u32
}

/// Fires once per change: when the current stable run (frames within `threshold` of the run's
/// first frame) has lasted `settle` ticks and differs from the last triggered frame by more than `threshold`.
pub struct ChangeGate {
    threshold: u32,
    settle: u32,
    anchor: Option<Fingerprint>,
    stable: u32,
    triggered: Option<Fingerprint>,
}

impl ChangeGate {
    pub fn new(threshold: u32, settle: u32) -> Self {
        Self { threshold, settle, anchor: None, stable: 0, triggered: None }
    }

    pub fn tick(&mut self, fp: Fingerprint) -> bool {
        match &self.anchor {
            Some(a) if distance(a, &fp) <= self.threshold => self.stable = self.stable.saturating_add(1),
            _ => {
                self.anchor = Some(fp.clone());
                self.stable = 0;
            }
        }
        let changed = self.triggered.as_ref().is_none_or(|t| distance(t, &fp) > self.threshold);
        if changed && self.stable >= self.settle {
            self.triggered = Some(fp);
            return true;
        }
        false
    }
}

/// A translated line positioned in logical pixels.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct OverlayLine {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub text: String,
    pub font_px: f64,
}

/// Convert physical line boxes to logical overlay lines, dropping blank translations.
/// Output coords are logical px relative to the captured region's origin, so the overlay window must sit at the region.
pub fn layout(lines: &[(PhysicalRect, String)], scale: f64) -> Vec<OverlayLine> {
    debug_assert!(scale > 0.0);
    lines
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(r, text)| {
            let height = r.height as f64 / scale;
            OverlayLine {
                x: r.x as f64 / scale,
                y: r.y as f64 / scale,
                width: r.width as f64 / scale,
                height,
                text: text.clone(),
                font_px: (height * 0.75).clamp(10.0, 48.0),
            }
        })
        .collect()
}
