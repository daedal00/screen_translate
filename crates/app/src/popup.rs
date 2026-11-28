//! Translation popup: shows "Translating…", then the result, with Copy / Settings / Close.

use gpui::{
    App, Context, EventEmitter, FocusHandle, IntoElement, KeyDownEvent, Rgba, SharedString, Window,
    WindowAppearance, div, prelude::*, px, rgb,
};
use screen_translate_core::pipeline::RegionResult;

/// Popup window size in logical px.
pub const POPUP_SIZE: (f64, f64) = (380.0, 220.0);

const PENDING_TEXT: &str = "Translating\u{2026}";

#[derive(Clone, Debug, PartialEq)]
pub enum PopupEvent {
    Close,
    OpenSettings,
    Copy(String),
}

pub struct PopupView {
    pending: bool,
    result: Option<RegionResult>,
    copied: bool,
    /// Blur closes the popup, but only after it has had focus since it was shown.
    armed: bool,
    focus: Option<FocusHandle>,
}

impl EventEmitter<PopupEvent> for PopupView {}

impl Default for PopupView {
    fn default() -> Self {
        Self::new()
    }
}

struct Colors {
    bg: Rgba,
    fg: Rgba,
    muted: Rgba,
    line: Rgba,
    accent: Rgba,
}

impl Colors {
    fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                bg: rgb(0x232326),
                fg: rgb(0xf1f1f1),
                muted: rgb(0x9a9a9f),
                line: rgb(0x3a3a3f),
                accent: rgb(0x6b9bff),
            },
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                bg: rgb(0xffffff),
                fg: rgb(0x1a1a1a),
                muted: rgb(0x6b6b6b),
                line: rgb(0xd8d8d8),
                accent: rgb(0x2b6cff),
            },
        }
    }
}

impl PopupView {
    pub fn new() -> Self {
        Self {
            pending: true,
            result: None,
            copied: false,
            armed: false,
            focus: None,
        }
    }

    /// Back to "Translating…": drops the previous content.
    pub fn pending(&mut self) {
        self.pending = true;
        self.result = None;
        self.copied = false;
        self.armed = false;
    }

    pub fn show(&mut self, r: RegionResult) {
        self.pending = false;
        self.result = Some(r);
        self.copied = false;
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn translation_text(&self) -> Option<String> {
        if self.pending {
            return Some(PENDING_TEXT.into());
        }
        self.result.as_ref()?.translation.clone()
    }

    pub fn error_text(&self) -> Option<String> {
        self.result.as_ref()?.error.clone()
    }

    pub fn original_text(&self) -> String {
        self.result
            .as_ref()
            .map(|r| r.original.clone())
            .unwrap_or_default()
    }

    pub fn source_label(&self) -> String {
        self.result
            .as_ref()
            .and_then(|r| r.detected_source.as_ref())
            .map(|s| format!("From {s}"))
            .unwrap_or_default()
    }

    pub fn copy_label(&self) -> &str {
        if self.copied { "Copied" } else { "Copy" }
    }

    pub fn copy_visible(&self) -> bool {
        self.result
            .as_ref()
            .is_some_and(|r| r.translation.is_some())
    }

    pub fn settings_visible(&self) -> bool {
        self.result.as_ref().is_some_and(|r| r.needs_settings)
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PopupEvent::Close);
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        cx.emit(PopupEvent::OpenSettings);
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.result.as_ref().and_then(|r| r.translation.clone()) else {
            return;
        };
        self.copied = true;
        cx.emit(PopupEvent::Copy(text));
        cx.notify();
    }

    /// Arm blur-close without waiting for the first activation (the popup never became key).
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// Window focus changed: the first focus arms blur-close, a later blur closes.
    pub fn activation_changed(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            self.armed = true;
        } else if self.armed {
            self.close(cx);
        }
    }
}

fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    c: &Colors,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(10.0))
        .py(px(3.0))
        .text_color(c.fg)
        .border_1()
        .border_color(c.line)
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.border_color(c.accent))
        .child(label.into())
        .on_click(move |_, window, cx| on_click(window, cx))
}

impl Render for PopupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::for_appearance(window.appearance());
        // Escape needs key focus: take it lazily (the view is built without a context).
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if !focus.is_focused(window) {
            window.focus(&focus, cx);
        }

        let mut content = div()
            .id("content")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if let Some(t) = self.translation_text() {
            let color = if self.pending { c.muted } else { c.fg };
            content = content.child(
                div()
                    .text_size(px(18.0))
                    .line_height(px(25.0))
                    .text_color(color)
                    .child(t),
            );
        }
        if let Some(e) = self.error_text() {
            content = content.child(div().text_color(rgb(0xdd3333)).child(e));
        }
        let original = self.original_text();
        if !original.is_empty() {
            content = content.child(
                div()
                    .mt(px(10.0))
                    .text_size(px(12.0))
                    .text_color(c.muted)
                    .child(original),
            );
        }

        let mut bar = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .mt(px(8.0))
            .text_size(px(12.0))
            .text_color(c.muted)
            .child(div().flex_1().child(self.source_label()));
        if self.settings_visible() {
            bar = bar.child(button("settings", "Settings", &c, {
                let this = cx.entity();
                move |_, cx| this.update(cx, |v, cx| v.open_settings(cx))
            }));
        }
        if self.copy_visible() {
            bar = bar.child(button("copy", self.copy_label().to_string(), &c, {
                let this = cx.entity();
                move |_, cx| this.update(cx, |v, cx| v.copy(cx))
            }));
        }
        bar = bar.child(button("close", "Close", &c, {
            let this = cx.entity();
            move |_, cx| this.update(cx, |v, cx| v.close(cx))
        }));

        div()
            .size_full()
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
            .flex()
            .flex_col()
            .px(px(14.0))
            .py(px(12.0))
            .bg(c.bg)
            .text_color(c.fg)
            .text_size(px(14.0))
            .border_1()
            .border_color(c.line)
            .child(content)
            .child(bar)
    }
}
