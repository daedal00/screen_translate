//! One-shot translate flow: selector -> capture -> popup. gpui has no public hide/show or move for windows,
//! so the selector and popup are created on demand and removed when done (not created once and reused).

use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::unbounded;

use gpui::{
    App, AppContext, Bounds, ClipboardItem, DisplayId, Entity, Global, Pixels,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, point, px,
    size,
};
use screen_translate_app::live::{self, LiveView};
use screen_translate_app::popup::{PopupEvent, PopupView};
use screen_translate_app::selector::{SelectorEvent, SelectorView};
use screen_translate_core::capture::{self, LogicalRect};
use screen_translate_core::live::OverlayLine;
use screen_translate_core::pipeline::{RegionResult, SELECTION_EMPTY, Target};

use crate::{AppState, native};
use screen_translate_app::overlay;
use screen_translate_app::window::remove_after_key_up;

/// One composited frame, so a removed selector is really gone before capture.
const HIDE_SETTLE: Duration = Duration::from_millis(30);
/// Polls (every `HIDE_POLL`, ~500ms in total) for the selector window to be gone.
const HIDE_POLL: Duration = Duration::from_millis(10);
const HIDE_WAIT_POLLS: u32 = 50;
const LIVE_TICK: Duration = Duration::from_millis(500);
/// Pause after a failed live fire, so a persistent failure doesn't hammer the provider.
const LIVE_BACKOFF: Duration = Duration::from_secs(2);

/// The open overlay windows and the monitor the selector covers. Both windows are `None` when closed.
#[derive(Default)]
pub(crate) struct Overlays {
    selector: Option<(WindowHandle<SelectorView>, Entity<SelectorView>)>,
    popup: Option<WindowHandle<PopupView>>,
    popup_view: Option<Entity<PopupView>>,
    target: Option<Target>,
    /// The selector was opened for a live session: Submit starts one instead of a one-shot translate.
    live: bool,
    live_window: Option<(WindowHandle<LiveView>, Entity<LiveView>)>,
}

impl Global for Overlays {}

fn to_bounds(r: LogicalRect) -> Bounds<Pixels> {
    Bounds::new(
        point(px(r.x as f32), px(r.y as f32)),
        size(px(r.width as f32), px(r.height as f32)),
    )
}

/// The gpui display of `target`: gpui's `DisplayId` is the CGDirectDisplayID (macOS) / HMONITOR (Windows), which is xcap's monitor id.
fn display_for(cx: &App, target: &Target) -> Option<DisplayId> {
    cx.displays()
        .iter()
        .map(|d| d.id())
        .find(|id| u64::from(*id) as u32 == target.display_id)
}

fn overlay_options(
    bounds: Bounds<Pixels>,
    display_id: Option<DisplayId>,
    transparent: bool,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: if transparent {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        },
        ..Default::default()
    }
}

/// Removes a window once the current update (possibly its own event handler) has finished.
fn close<V: 'static>(handle: WindowHandle<V>, cx: &mut App) {
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    });
}

/// The window's view, or closes the window (nothing tracks it yet) when it can't be read.
fn entity_or_close<V: gpui::Render>(
    handle: WindowHandle<V>,
    cx: &mut App,
) -> Result<Entity<V>, String> {
    handle.entity(cx).map_err(|e| {
        close(handle, cx);
        e.to_string()
    })
}

/// Hide the popup and drop its content. Screen content is private; the invalidation also stops an
/// in-flight translation that finishes later from being stored or shown.
pub fn hide_popup(cx: &mut App) {
    if let Some(handle) = take_popup(cx) {
        close(handle, cx);
    }
}

/// Drops the popup's state now and returns its window for the caller to remove.
fn take_popup(cx: &mut App) -> Option<WindowHandle<PopupView>> {
    cx.global::<AppState>().latest.invalidate();
    let o = cx.global_mut::<Overlays>();
    o.popup_view = None;
    o.popup.take()
}

/// Open the selector on the monitor under the cursor. A second call while it is open does nothing,
/// which also swallows hotkey auto-repeat.
pub fn start_selection(cx: &mut App, live: bool) {
    if cx.has_global::<Overlays>() && cx.global::<Overlays>().selector.is_some() {
        return;
    }
    if let Err(e) = open_selector(cx, live) {
        log::error!("start_selection failed: {e}");
    }
}

fn open_selector(cx: &mut App, live: bool) -> Result<(), String> {
    if !cx.has_global::<Overlays>() {
        cx.set_global(Overlays::default());
    }
    hide_popup(cx);
    let cursor = native::cursor_position().unwrap_or((0, 0));
    let target = Target::at_point(cursor)?;
    let bounds = to_bounds(overlay::selector_bounds(&target, overlay::GLOBAL_BOUNDS));
    let options = overlay_options(bounds, display_for(cx, &target), true);
    let handle = cx
        .open_window(options, |_, cx| cx.new(|_| SelectorView::new()))
        .map_err(|e| e.to_string())?;
    let view = entity_or_close(handle, cx)?;
    let excluded = handle.update(cx, |_, window, _| {
        window.activate_window();
        if let Err(e) = native::cover_screen(window, None) {
            log::warn!("covering the screen failed: {e}");
        }
        native::exclude_from_capture(window)
    });
    if let Err(e) = excluded.map_err(|e| e.to_string()).and_then(|r| r) {
        log::warn!(
            "excluding the selector from capture failed (relying on hide-before-capture): {e}"
        );
    }
    cx.subscribe(&view, |emitter, event: &SelectorEvent, cx| {
        // A cancelled selector lingers for the key-up grace; its late events must not touch a newer one.
        let current = cx
            .global::<Overlays>()
            .selector
            .as_ref()
            .map(|(_, v)| v.entity_id());
        if current != Some(emitter.entity_id()) {
            return;
        }
        match event {
            SelectorEvent::Cancel => {
                let o = cx.global_mut::<Overlays>();
                o.target = None;
                if let Some((handle, _)) = o.selector.take() {
                    remove_after_key_up(handle, cx);
                }
            }
            SelectorEvent::Submit(rect) => submit(*rect, cx),
        }
    })
    .detach();
    let o = cx.global_mut::<Overlays>();
    o.selector = Some((handle, view));
    o.target = Some(target);
    o.live = live;
    Ok(())
}

fn close_selector(cx: &mut App) {
    if let Some((handle, _)) = cx.global_mut::<Overlays>().selector.take() {
        close(handle, cx);
    }
}

/// Close the selector, capture `rect` (selector-local logical px), show the popup, OCR + translate.
/// An empty selection keeps the selector open so the user can drag again.
pub fn submit(rect: LogicalRect, cx: &mut App) {
    let Some(target) = cx.global::<Overlays>().target else {
        return;
    };
    if let Err(e) = target.check(rect) {
        debug_assert_eq!(e, SELECTION_EMPTY);
        if let Some((_, view)) = &cx.global::<Overlays>().selector {
            view.clone().update(cx, |v, cx| {
                v.reset();
                cx.notify();
            });
        }
        return;
    }
    if cx.global::<Overlays>().live {
        close_selector(cx);
        if let Err(e) = start_live(cx, &target, rect) {
            log::error!("starting live failed: {e}");
        }
        return;
    }
    let selector_id = cx
        .global::<Overlays>()
        .selector
        .as_ref()
        .map(|(h, _)| h.window_id());
    close_selector(cx);
    let state = cx.global::<AppState>();
    let generation = state.latest.begin();
    let (rt, translator) = (state.runtime.handle().clone(), state.translator.clone());
    cx.spawn(async move |cx| {
        // Wait (bounded) until the selector window is really gone, then let one frame composite before capturing.
        for _ in 0..HIDE_WAIT_POLLS {
            if !cx.update(|cx| {
                cx.windows()
                    .iter()
                    .any(|w| Some(w.window_id()) == selector_id)
            }) {
                break;
            }
            cx.background_executor().timer(HIDE_POLL).await;
        }
        cx.background_executor().timer(HIDE_SETTLE).await;
        let (origin, scale) = (target.origin, target.scale);
        let captured = match rt
            .spawn_blocking(move || capture::grab_region(origin, rect, scale))
            .await
        {
            Ok(r) => r,
            Err(e) => Err(e.to_string()),
        };
        // A newer request or a hide superseded this one while capturing.
        if !cx.update(|cx| cx.global::<AppState>().latest.is_current(generation)) {
            return;
        }
        let img = match captured {
            Ok(Some(img)) => img,
            other => {
                let msg = other.err().unwrap_or_else(|| "Selection is empty".into());
                cx.update(|cx| {
                    show_popup(cx, &target, rect);
                    finish(cx, generation, RegionResult::failed(String::new(), msg));
                });
                return;
            }
        };
        cx.update(|cx| show_popup(cx, &target, rect));
        let result = rt
            .spawn(async move { translator.translate_crop(img).await })
            .await
            .unwrap_or_else(|e| RegionResult::failed(String::new(), e.to_string()));
        cx.update(|cx| finish(cx, generation, result));
    })
    .detach();
}

/// Deliver `result` to the popup unless a newer request (or a hide) superseded `generation`.
fn finish(cx: &mut App, generation: u64, result: RegionResult) {
    let Some(view) = cx.global::<Overlays>().popup_view.clone() else {
        return;
    };
    let latest = cx.global::<AppState>().latest.clone();
    latest.finish_with(generation, result, |r| {
        log::debug!("result: {r:?}");
        view.update(cx, |v, cx| {
            v.show(r.clone());
            cx.notify();
        });
    });
}

/// Show the popup next to the selection in its "translating" state.
fn show_popup(cx: &mut App, target: &Target, rect: LogicalRect) {
    if let Err(e) = open_popup(cx, target, rect) {
        log::error!("showing the popup failed: {e}");
    }
}

fn open_popup(cx: &mut App, target: &Target, rect: LogicalRect) -> Result<(), String> {
    // `rect` is relative to the monitor (the selector fills it), so place in monitor-local coords.
    let bounds = to_bounds(overlay::popup_bounds(target, rect, overlay::GLOBAL_BOUNDS));
    let display_id = display_for(cx, target);
    let handle = cx
        .open_window(overlay_options(bounds, display_id, false), |window, cx| {
            cx.new(|cx| {
                // Blur closes the popup, but only after it has had focus (see `PopupView::activation_changed`).
                cx.observe_window_activation(window, |this: &mut PopupView, window, cx| {
                    this.activation_changed(window.is_window_active(), cx)
                })
                .detach();
                PopupView::new()
            })
        })
        .map_err(|e| e.to_string())?;
    let view = entity_or_close(handle, cx)?;
    let armed = view.clone();
    let activated = handle.update(cx, |_, window, _| {
        window.activate_window();
        // Not key by the next frame: arm blur-close anyway so a later blur can still dismiss it.
        window.on_next_frame(move |window, cx| {
            if !window.is_window_active() {
                armed.update(cx, |v, _| v.arm());
            }
        });
    });
    if let Err(e) = activated {
        log::warn!("popup activation failed: {e}");
    }
    cx.subscribe(&view, |emitter, event: &PopupEvent, cx| {
        let current = cx
            .global::<Overlays>()
            .popup_view
            .as_ref()
            .map(Entity::entity_id);
        if current != Some(emitter.entity_id()) {
            return;
        }
        match event {
            PopupEvent::Close => {
                if let Some(handle) = take_popup(cx) {
                    remove_after_key_up(handle, cx);
                }
            }
            PopupEvent::OpenSettings => crate::settings_window::open(cx),
            PopupEvent::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
            }
        }
    })
    .detach();
    let o = cx.global_mut::<Overlays>();
    o.popup = Some(handle);
    o.popup_view = Some(view);
    Ok(())
}

/// Hotkey in Live mode: stop the running session, else pick a region for a new one.
pub fn toggle_live(cx: &mut App) {
    if cx.global::<AppState>().live.running() {
        stop_live(cx);
    } else {
        start_selection(cx, true);
    }
}

/// End the session: the overlay is cleared and closed, and no later update can show.
pub fn stop_live(cx: &mut App) {
    let view = cx
        .global::<Overlays>()
        .live_window
        .as_ref()
        .map(|(_, v)| v.clone());
    let live = cx.global::<AppState>().live.clone();
    // `stop_with` runs under the Live lock, so no update from the loop can be forwarded after it.
    if !live.stop_with(|| ()) {
        return;
    }
    if let Some(view) = view {
        view.update(cx, |v, cx| {
            v.clear();
            cx.notify();
        });
    }
    if let Some((handle, _)) = cx.global_mut::<Overlays>().live_window.take() {
        close(handle, cx);
    }
    log::info!("live stopped");
}

/// Open the overlay over `rect` (monitor-local logical px), claim a session and run the loop on its own thread.
fn start_live(cx: &mut App, target: &Target, rect: LogicalRect) -> Result<(), String> {
    let bounds = to_bounds(overlay::display_bounds(
        target,
        rect,
        overlay::GLOBAL_BOUNDS,
    ));
    let mut options = overlay_options(bounds, display_for(cx, target), true);
    options.focus = false;
    let handle = cx
        .open_window(options, |_, cx| cx.new(|_| LiveView::new()))
        .map_err(|e| e.to_string())?;
    let view = entity_or_close(handle, cx)?;
    let setup = handle.update(cx, |_, window, _| {
        let (x, y, w, h) = live::overlay_bounds(target, rect);
        if let Err(e) = native::cover_screen(window, Some((x, y, w, h))) {
            log::warn!("pinning the overlay frame failed: {e}");
        }
        if let Err(e) = native::click_through(window) {
            log::warn!("making the overlay click-through failed: {e}");
        }
        native::exclude_from_capture(window)
    });
    if let Err(e) = setup.map_err(|e| e.to_string()).and_then(|r| r) {
        log::warn!("excluding the overlay from capture failed: {e}");
    }
    // A previous session can only be left over if its stop was skipped; end it first.
    stop_live(cx);
    cx.global_mut::<Overlays>().live_window = Some((handle, view.clone()));

    let state = cx.global::<AppState>();
    let (live, translator, rt) = (
        state.live.clone(),
        state.translator.clone(),
        state.runtime.handle().clone(),
    );
    let session = live.start();
    let (tx, mut rx) = unbounded::<(Vec<OverlayLine>, Option<String>)>();
    let target = *target;
    std::thread::spawn(move || {
        let mut grabber = capture::RegionGrabber::new(target.origin, target.scale);
        let grab = move || grabber.grab(rect);
        let mut recognizer = screen_translate_core::ocr::LiveRecognizer::default();
        let on_update = |lines, error| {
            let _ = tx.unbounded_send((lines, error));
        };
        screen_translate_core::pipeline::run_live(
            &translator,
            &live,
            session,
            rect,
            LIVE_TICK,
            LIVE_BACKOFF,
            &rt,
            grab,
            move |img| recognizer.recognize(img),
            on_update,
        );
    });
    cx.spawn(async move |cx| {
        while let Some((lines, error)) = rx.next().await {
            cx.update(|cx| {
                // Only the current session may draw; `stop` ran on this thread before a late update arrives.
                if !cx.global::<AppState>().live.is_current(session) {
                    return;
                }
                view.update(cx, |v, cx| {
                    v.set_lines(lines);
                    if let Some(e) = error {
                        v.set_error(e);
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
    log::info!("live started on {rect:?}");
    Ok(())
}

/// Debug-only: env var `ST_SELFTEST` = "x,y,w,h" drives selector -> region submit without input.
#[cfg(debug_assertions)]
pub fn selftest(cx: &mut App) {
    let (live, var) = match std::env::var("ST_LIVE_SELFTEST") {
        Ok(_) => (true, "ST_LIVE_SELFTEST"),
        Err(_) => (false, "ST_SELFTEST"),
    };
    let Ok(spec) = std::env::var(var) else { return };
    let v: Vec<f64> = spec
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    let [x, y, width, height] = v[..] else {
        log::error!("selftest: {var} must be x,y,w,h");
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        cx.update(|cx| start_selection(cx, live));
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        log::info!("selftest: submitting {x},{y} {width}x{height}");
        cx.update(|cx| {
            submit(
                LogicalRect {
                    x,
                    y,
                    width,
                    height,
                },
                cx,
            )
        });
    })
    .detach();
}
