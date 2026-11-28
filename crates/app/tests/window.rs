//! A delayed key-up removal closes only its own window, however many times it is requested.
use gpui::{AppContext, TestAppContext, WindowHandle};
use screen_translate_app::selector::SelectorView;
use screen_translate_app::window::{KEY_UP_GRACE, remove_after_key_up};

fn open(cx: &mut TestAppContext) -> WindowHandle<SelectorView> {
    cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| SelectorView::new()))
            .unwrap()
    })
}

#[gpui::test]
fn stale_timer_does_not_close_a_newer_window(cx: &mut TestAppContext) {
    let old = open(cx);
    cx.update(|cx| {
        remove_after_key_up(old, cx);
        remove_after_key_up(old, cx); // double Escape
    });
    cx.executor().advance_clock(KEY_UP_GRACE / 2);
    let new = open(cx); // hotkey inside the grace window
    cx.executor().advance_clock(KEY_UP_GRACE);
    cx.run_until_parked();
    let ids: Vec<_> = cx.update(|cx| cx.windows().iter().map(|w| w.window_id()).collect());
    assert_eq!(ids, vec![new.window_id()]);
}
