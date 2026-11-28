//! Pure core of live mode: change detection and overlay layout.

use std::time::Duration;

use image::RgbaImage;

use crate::capture::PhysicalRect;
use crate::ocr::TextLine;

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
    a.0.iter()
        .zip(&b.0)
        .filter(|(x, y)| (**x - **y).abs() > CELL_DELTA)
        .count() as u32
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
        Self {
            threshold,
            settle,
            anchor: None,
            stable: 0,
            triggered: None,
        }
    }

    pub fn tick(&mut self, fp: Fingerprint) -> bool {
        match &self.anchor {
            Some(a) if distance(a, &fp) <= self.threshold => {
                self.stable = self.stable.saturating_add(1)
            }
            _ => {
                self.anchor = Some(fp.clone());
                self.stable = 0;
            }
        }
        let changed = self
            .triggered
            .as_ref()
            .is_none_or(|t| distance(t, &fp) > self.threshold);
        if changed && self.stable >= self.settle {
            self.triggered = Some(fp);
            return true;
        }
        false
    }
}

/// Smallest font `layout` shrinks a long translation to before the box clips it.
const LAYOUT_MIN_FONT_PX: f64 = 8.0;

/// Horizontal padding LiveView adds on both sides of a line (2 px each).
const LIVE_PADDING_PX: f64 = 4.0;

/// Kana, han, hangul, CJK punctuation and full-width forms: one em per glyph, and a line may break
/// anywhere between them (see `split_translation` for the punctuation that may not start a line).
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{1100}'..='\u{11FF}' | '\u{3000}'..='\u{30FF}' | '\u{3130}'..='\u{318F}'
        | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{FF00}'..='\u{FFEF}')
}

/// Font size for `text` in a `box_w` x `box_h` box: the box height's size, shrunk until the text
/// fits the width on one line (estimated: 1 em per CJK glyph, 0.55 em otherwise), never below `min_px`.
pub fn fit_font_px(text: &str, box_w: f64, box_h: f64, min_px: f64) -> f64 {
    let height_size = (box_h * 0.75).clamp(10.0, 48.0);
    let em: f64 = text
        .chars()
        .map(|c| if is_cjk(c) { 1.0 } else { 0.55 })
        .sum();
    if em == 0.0 {
        return height_size;
    }
    min_px.max(height_size.min(box_w / em))
}

/// Pause after `failures` consecutive failures: `base` doubling per failure, capped at `max`.
pub fn backoff(failures: u32, base: Duration, max: Duration) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    base.saturating_mul(1 << (failures - 1).min(31)).min(max)
}

/// Group OCR lines into text blocks (paragraphs, subtitles): a line joins the block of a line just
/// above it when the gap is under 0.6 line heights and the two overlap horizontally or are left-aligned.
/// Blocks come in reading order (by topmost line); indices within a block run top to bottom.
pub fn group_blocks(lines: &[TextLine]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by_key(|&i| (lines[i].bbox.y, lines[i].bbox.x));
    let mut root: Vec<usize> = (0..lines.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for (n, &b) in order.iter().enumerate() {
        for &a in &order[..n] {
            let (ra, rb) = (lines[a].bbox, lines[b].bbox);
            let h = ra.height.min(rb.height) as f64;
            let gap = rb.y as f64 - (ra.y + ra.height) as f64;
            let overlap = ra.x < rb.x + rb.width && rb.x < ra.x + ra.width;
            let aligned = (ra.x as f64 - rb.x as f64).abs() <= h;
            // `b` must sit below `a`, not beside it on the same row.
            let below = rb.y as f64 >= ra.y as f64 + h * 0.5;
            if below && gap < h * 0.6 && (overlap || aligned) {
                let (x, y) = (find(&mut root, a), find(&mut root, b));
                root[y] = x;
            }
        }
    }
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    let mut slot = std::collections::HashMap::new();
    for &i in &order {
        let r = find(&mut root, i);
        let k = *slot.entry(r).or_insert_with(|| {
            blocks.push(Vec::new());
            blocks.len() - 1
        });
        blocks[k].push(i);
    }
    blocks
}

/// Closing punctuation that must not begin a line (kinsoku).
const NO_LINE_START: [char; 17] = [
    '、', '。', '，', '．', '」', '』', '）', '】', '〉', '》', '！', '？', '：', '；', ')', '!',
    '?',
];

/// Split `translated` into `line_lens.len()` lines, each roughly proportional to its source line's
/// length. Breaks fall at whitespace, or between CJK glyphs; lines are trimmed.
pub fn split_translation(translated: &str, line_lens: &[usize]) -> Vec<String> {
    let chars: Vec<char> = translated.chars().collect();
    let n = chars.len();
    let total: usize = line_lens.iter().sum();
    let boundary = |p: usize| {
        p == 0
            || p == n
            || (!NO_LINE_START.contains(&chars[p])
                && (chars[p - 1].is_whitespace()
                    || chars[p].is_whitespace()
                    || is_cjk(chars[p - 1])
                    || is_cjk(chars[p])))
    };
    let mut out = Vec::with_capacity(line_lens.len());
    let (mut start, mut cum) = (0, 0);
    for (i, len) in line_lens.iter().enumerate() {
        let end = if i + 1 == line_lens.len() {
            n
        } else {
            cum += if total == 0 { 1 } else { *len };
            let denom = if total == 0 { line_lens.len() } else { total };
            let ideal = (n as f64 * cum as f64 / denom as f64).round() as usize;
            // Nearest break after `start` that leaves this line some visible text.
            (start + 1..=n)
                .filter(|&p| boundary(p) && chars[start..p].iter().any(|c| !c.is_whitespace()))
                .min_by_key(|&p| p.abs_diff(ideal))
                .unwrap_or(n)
        };
        out.push(
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_owned(),
        );
        start = end;
    }
    out
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
                font_px: fit_font_px(
                    text,
                    (r.width as f64 / scale - LIVE_PADDING_PX).max(0.0),
                    height,
                    LAYOUT_MIN_FONT_PX,
                ),
            }
        })
        .collect()
}
