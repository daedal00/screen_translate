//! Milestone 3a RED: selector drag state machine. Needs `screen_translate_app::selector`.
//! Implementer notes: SelectorView must fill the window and handle mouse down/move/up on the root
//! (the test window is maximized, so any in-window point hits it). Escape must reach the view without the
//! test focusing anything (use `window.on_key_event`, or focus lazily in render; the test runs until parked).
use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use screen_translate_app::selector::{SelectorEvent, SelectorView};
use screen_translate_core::capture::LogicalRect;

fn rect(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
    LogicalRect {
        x,
        y,
        width,
        height,
    }
}

fn setup(
    cx: &mut TestAppContext,
) -> (
    Entity<SelectorView>,
    &mut VisualTestContext,
    Rc<RefCell<Vec<SelectorEvent>>>,
) {
    let (view, cx) = cx.add_window_view(|_, _| SelectorView::new());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &SelectorEvent, _| {
            sink.borrow_mut().push(e.clone())
        })
        .detach();
    });
    cx.run_until_parked();
    (view, cx, events)
}

fn down(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_down(point(px(x), px(y)), MouseButton::Left, Modifiers::default());
}
fn mv(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_move(point(px(x), px(y)), MouseButton::Left, Modifiers::default());
}
fn up(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_up(point(px(x), px(y)), MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn drag_down_right_submits_rect(cx: &mut TestAppContext) {
    let (view, cx, events) = setup(cx);
    down(cx, 100.0, 120.0);
    mv(cx, 200.0, 180.0);
    assert_eq!(
        view.read_with(cx, |v, _| v.drag_rect()),
        Some(rect(100.0, 120.0, 100.0, 60.0))
    );
    up(cx, 300.0, 220.0);
    assert_eq!(
        *events.borrow(),
        vec![SelectorEvent::Submit(rect(100.0, 120.0, 200.0, 100.0))]
    );
    assert_eq!(view.read_with(cx, |v, _| v.drag_rect()), None);
}

#[gpui::test]
fn drag_up_left_normalizes_rect(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    down(cx, 300.0, 220.0);
    mv(cx, 150.0, 150.0);
    up(cx, 100.0, 120.0);
    assert_eq!(
        *events.borrow(),
        vec![SelectorEvent::Submit(rect(100.0, 120.0, 200.0, 100.0))]
    );
}

#[gpui::test]
fn drag_up_right_and_down_left_normalize(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    down(cx, 100.0, 220.0);
    up(cx, 300.0, 120.0);
    down(cx, 300.0, 120.0);
    up(cx, 100.0, 220.0);
    let r = rect(100.0, 120.0, 200.0, 100.0);
    assert_eq!(
        *events.borrow(),
        vec![SelectorEvent::Submit(r), SelectorEvent::Submit(r)]
    );
}

#[gpui::test]
fn tiny_drag_emits_nothing_and_resets(cx: &mut TestAppContext) {
    let (view, cx, events) = setup(cx);
    down(cx, 100.0, 100.0);
    mv(cx, 101.0, 101.0);
    assert!(view.read_with(cx, |v, _| v.drag_rect()).is_some());
    up(cx, 102.0, 102.0);
    assert!(events.borrow().is_empty());
    assert_eq!(view.read_with(cx, |v, _| v.drag_rect()), None);
}

#[gpui::test]
fn one_thin_axis_is_too_small(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    down(cx, 100.0, 100.0);
    up(cx, 400.0, 102.0); // 300 x 2
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn exactly_3x3_submits(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    down(cx, 10.0, 10.0);
    up(cx, 13.0, 13.0);
    assert_eq!(
        *events.borrow(),
        vec![SelectorEvent::Submit(rect(10.0, 10.0, 3.0, 3.0))]
    );
}

#[gpui::test]
fn right_button_is_ignored(cx: &mut TestAppContext) {
    let (view, cx, events) = setup(cx);
    cx.simulate_mouse_down(
        point(px(100.0), px(100.0)),
        MouseButton::Right,
        Modifiers::default(),
    );
    assert_eq!(view.read_with(cx, |v, _| v.drag_rect()), None);
    cx.simulate_mouse_up(
        point(px(300.0), px(300.0)),
        MouseButton::Right,
        Modifiers::default(),
    );
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn mouse_up_without_down_emits_nothing(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    up(cx, 300.0, 300.0);
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn escape_cancels(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(*events.borrow(), vec![SelectorEvent::Cancel]);
}

#[gpui::test]
fn reset_clears_in_progress_drag(cx: &mut TestAppContext) {
    let (view, cx, events) = setup(cx);
    down(cx, 100.0, 100.0);
    mv(cx, 250.0, 250.0);
    view.update(cx, |v, _| v.reset());
    assert_eq!(
        view.read_with(cx, |v, _| (v.drag_rect(), v.size_label())),
        (None, None)
    );
    up(cx, 300.0, 300.0);
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn size_label_is_rounded_with_times_sign(cx: &mut TestAppContext) {
    let (view, cx, _events) = setup(cx);
    assert_eq!(view.read_with(cx, |v, _| v.size_label()), None);
    down(cx, 100.0, 100.0);
    mv(cx, 340.0, 250.0);
    assert_eq!(
        view.read_with(cx, |v, _| v.size_label()),
        Some("240 \u{d7} 150".to_string())
    );
}

#[gpui::test]
fn label_inside_when_no_room_below(cx: &mut TestAppContext) {
    let (view, cx, _events) = setup(cx);
    down(cx, 100.0, 100.0);
    mv(cx, 200.0, 200.0); // rect.y + h = 200; 200 + 28 = 228
    assert!(!view.read_with(cx, |v, _| v.label_inside(1000.0)));
    assert!(!view.read_with(cx, |v, _| v.label_inside(228.0))); // not strictly greater
    assert!(view.read_with(cx, |v, _| v.label_inside(227.0)));
}
