//! Region selector: a transparent full-monitor window where the user drags a rectangle.

use gpui::{
    Context, EventEmitter, FocusHandle, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Render, Window, div, hsla, prelude::*, px, white,
};
use screen_translate_core::capture::LogicalRect;

/// Smallest drag (per axis, logical px) that counts as a selection.
const MIN_SIZE: f64 = 3.0;
/// Vertical room the size label needs below the box.
const LABEL_ROOM: f64 = 28.0;

/// A point in window-local logical px.
type Xy = (f64, f64);

#[derive(Clone, Debug, PartialEq)]
pub enum SelectorEvent {
    /// Rect in window-local logical px.
    Submit(LogicalRect),
    Cancel,
}

#[derive(Default)]
pub struct SelectorView {
    /// (start, current) of the drag in progress.
    drag: Option<(Xy, Xy)>,
    focus: Option<FocusHandle>,
}

impl EventEmitter<SelectorEvent> for SelectorView {}

impl SelectorView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.drag = None;
    }

    pub fn drag_rect(&self) -> Option<LogicalRect> {
        let ((x0, y0), (x1, y1)) = self.drag?;
        Some(LogicalRect {
            x: x0.min(x1),
            y: y0.min(y1),
            width: (x1 - x0).abs(),
            height: (y1 - y0).abs(),
        })
    }

    /// "W × H", rounded.
    pub fn size_label(&self) -> Option<String> {
        let r = self.drag_rect()?;
        Some(format!("{} \u{d7} {}", r.width.round(), r.height.round()))
    }

    /// True when there is no room for the label below the box, so it goes inside.
    pub fn label_inside(&self, window_height: f64) -> bool {
        self.drag_rect()
            .is_some_and(|r| r.y + r.height + LABEL_ROOM > window_height)
    }

    fn pos(p: gpui::Point<gpui::Pixels>) -> Xy {
        (f64::from(f32::from(p.x)), f64::from(f32::from(p.y)))
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let p = Self::pos(e.position);
        self.drag = Some((p, p));
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            self.move_to(Self::pos(e.position));
            cx.notify();
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.move_to(Self::pos(e.position));
        let rect = self.drag_rect();
        self.drag = None;
        cx.notify();
        if let Some(rect) = rect.filter(|r| r.width >= MIN_SIZE && r.height >= MIN_SIZE) {
            cx.emit(SelectorEvent::Submit(rect));
        }
    }

    fn move_to(&mut self, p: Xy) {
        if let Some((_, current)) = &mut self.drag {
            *current = p;
        }
    }
}

impl Render for SelectorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dim = hsla(0.0, 0.0, 0.0, 0.35);
        // Escape needs key focus: take it lazily (the view is built without a context).
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if !focus.is_focused(window) {
            window.focus(&focus, cx);
        }

        let root = div()
            .size_full()
            .relative()
            .track_focus(&focus)
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(SelectorEvent::Cancel);
                }
            }))
            .cursor_crosshair()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e, _, cx| this.mouse_down(e, cx)),
            )
            .on_mouse_move(cx.listener(|this, e, _, cx| this.mouse_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e, _, cx| this.mouse_up(e, cx)),
            );

        let (Some(r), Some(label)) = (self.drag_rect(), self.size_label()) else {
            return root.bg(dim);
        };
        let size = window.viewport_size();
        let (win_w, win_h) = (
            f64::from(f32::from(size.width)),
            f64::from(f32::from(size.height)),
        );
        let band = |x: f64, y: f64, w: f64, h: f64| {
            div()
                .absolute()
                .left(px(x as f32))
                .top(px(y as f32))
                .w(px(w.max(0.0) as f32))
                .h(px(h.max(0.0) as f32))
                .bg(dim)
        };
        let label_box = div()
            .absolute()
            .right(px(if self.label_inside(win_h) { 4.0 } else { -1.0 }))
            .px(px(6.0))
            .rounded(px(3.0))
            .bg(hsla(0.0, 0.0, 0.0, 0.75))
            .text_color(white())
            .text_size(px(11.0))
            .line_height(px(16.0))
            .whitespace_nowrap()
            .child(label);
        let label_box = if self.label_inside(win_h) {
            label_box.bottom(px(4.0))
        } else {
            label_box.top(px(r.height as f32 + 4.0))
        };
        root.child(band(0.0, 0.0, win_w, r.y))
            .child(band(0.0, r.y + r.height, win_w, win_h - r.y - r.height))
            .child(band(0.0, r.y, r.x, r.height))
            .child(band(r.x + r.width, r.y, win_w - r.x - r.width, r.height))
            .child(
                div()
                    .absolute()
                    .left(px(r.x as f32))
                    .top(px(r.y as f32))
                    .w(px(r.width as f32))
                    .h(px(r.height as f32))
                    .border_1()
                    .border_color(white())
                    .child(label_box),
            )
    }
}
