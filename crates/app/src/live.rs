//! Live translation overlay: translated lines drawn over the watched region.

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb};
use screen_translate_core::capture::LogicalRect;
use screen_translate_core::live::OverlayLine;
use screen_translate_core::pipeline::Target;

/// The overlay window's (x, y, w, h) in the platform's global units: logical points on macOS,
/// physical virtual-desktop px on Windows. The screen position of the watched region.
pub fn overlay_bounds(target: &Target, rect: LogicalRect) -> (f64, f64, f64, f64) {
    #[cfg(windows)]
    {
        let px = |v: f64| (v * target.scale).round();
        (
            target.pos.0 as f64 + px(rect.x),
            target.pos.1 as f64 + px(rect.y),
            px(rect.width),
            px(rect.height),
        )
    }
    #[cfg(not(windows))]
    {
        let r = crate::overlay::display_bounds(target, rect, true);
        (r.x, r.y, r.width, r.height)
    }
}

#[derive(Default)]
pub struct LiveView {
    lines: Vec<OverlayLine>,
    error: Option<String>,
}

impl LiveView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces everything shown, including an earlier error.
    pub fn set_lines(&mut self, lines: Vec<OverlayLine>) {
        self.lines = lines;
        self.error = None;
    }

    /// Shown on top of the current lines.
    pub fn set_error(&mut self, err: String) {
        self.error = Some(err);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.error = None;
    }

    pub fn lines(&self) -> &[OverlayLine] {
        &self.lines
    }

    pub fn error_text(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Render for LiveView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().relative().overflow_hidden();
        for l in &self.lines {
            root = root.child(
                div()
                    .absolute()
                    .left(px(l.x as f32))
                    .top(px(l.y as f32))
                    .w(px(l.width as f32))
                    .h(px(l.height as f32))
                    .px(px(2.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .bg(rgb(0x1c1c1e))
                    .text_color(rgb(0xf5f5f5))
                    .text_size(px(l.font_px as f32))
                    .line_height(px(l.height as f32))
                    .child(l.text.clone()),
            );
        }
        if let Some(e) = &self.error {
            // Inset 4px on every side like live.html (`max-width: calc(100% - 8px)`); the chip shrinks to fit.
            root = root.child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .left(px(4.0))
                    .right(px(4.0))
                    .flex()
                    .child(
                        div()
                            .min_w_0()
                            .px(px(6.0))
                            .py(px(2.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .rounded(px(3.0))
                            .bg(rgb(0x1c1c1e))
                            .text_color(rgb(0xff7b72))
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .child(e.clone()),
                    ),
            );
        }
        root
    }
}
