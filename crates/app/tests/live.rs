//! Milestone 3b RED: live overlay view state. Needs `screen_translate_app::live::LiveView`.
//! Not covered (skipped): line box bounds. gpui's `debug_bounds` needs `debug_selector`, which only exists
//! under gpui's `test-support` cfg and is not available to the lib in integration tests.
use gpui::TestAppContext;
use screen_translate_app::live::LiveView;
use screen_translate_core::live::OverlayLine;

fn line(x: f64, y: f64, text: &str) -> OverlayLine {
    OverlayLine {
        x,
        y,
        width: 120.0,
        height: 20.0,
        text: text.into(),
        font_px: 15.0,
    }
}

#[gpui::test]
fn new_is_empty(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| LiveView::new());
    view.read_with(cx, |v, _| {
        assert!(v.lines().is_empty());
        assert_eq!(v.error_text(), None);
    });
}

#[gpui::test]
fn set_lines_replaces_and_clears_error(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| LiveView::new());
    view.update(cx, |v, _| {
        v.set_lines(vec![line(1.0, 2.0, "a"), line(3.0, 4.0, "b")]);
        v.set_error("boom".into());
        v.set_lines(vec![line(5.0, 6.0, "c")]);
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.lines(), &[line(5.0, 6.0, "c")]);
        assert_eq!(v.error_text(), None);
    });
}

#[gpui::test]
fn set_error_keeps_lines(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| LiveView::new());
    view.update(cx, |v, _| {
        v.set_lines(vec![line(1.0, 2.0, "a")]);
        v.set_error("DeepL key missing".into());
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.lines(), &[line(1.0, 2.0, "a")]);
        assert_eq!(v.error_text(), Some("DeepL key missing"));
    });
}

#[gpui::test]
fn clear_removes_lines_and_error(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| LiveView::new());
    view.update(cx, |v, _| {
        v.set_lines(vec![line(1.0, 2.0, "a")]);
        v.set_error("x".into());
        v.clear();
    });
    view.read_with(cx, |v, _| {
        assert!(v.lines().is_empty());
        assert_eq!(v.error_text(), None);
    });
}

#[gpui::test]
fn renders_lines_and_error_without_panic(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| LiveView::new());
    view.update(cx, |v, _| {
        v.set_lines(vec![line(10.0, 20.0, "Hello"), line(10.0, 50.0, "World")])
    });
    cx.run_until_parked();
    view.update(cx, |v, _| v.set_error("oops".into()));
    cx.run_until_parked();
    view.update(cx, |v, _| v.clear());
    cx.run_until_parked();
}
