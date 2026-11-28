//! Window helpers shared by the app and its tests.

use std::time::Duration;

use gpui::{App, WindowHandle};

/// Lets the key-up of an Escape land before its window goes away.
pub const KEY_UP_GRACE: Duration = Duration::from_millis(150);

/// Removes `handle`'s window after `KEY_UP_GRACE`. Escape closes a window on key down; removing it right away
/// makes the key-up arrive for a window that no longer exists, which gpui logs as an error. Only this window
/// is affected: a window opened meanwhile is never touched, and removing one that is already gone is a no-op.
pub fn remove_after_key_up<V: 'static>(handle: WindowHandle<V>, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(KEY_UP_GRACE).await;
        let _ = cx.update(|cx| handle.update(cx, |_, window, _| window.remove_window()));
    })
    .detach();
}
