//! Platform glue gpui does not expose: global cursor position and excluding a window from screen capture.

use raw_window_handle::HasWindowHandle;

/// Cursor position in xcap's units (points, top-left origin on macOS; physical virtual-desktop px on Windows).
pub fn cursor_position() -> Option<(i32, i32)> {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSEvent, NSScreen};
        let p = NSEvent::mouseLocation();
        // Cocoa's origin is the bottom-left of the primary screen (the first one); flip into top-left coordinates.
        let primary_height = NSScreen::screens(MainThreadMarker::new()?)
            .firstObject()?
            .frame()
            .size
            .height;
        Some((p.x.round() as i32, (primary_height - p.y).round() as i32))
    }
    #[cfg(windows)]
    {
        use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some((p.x, p.y))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    None
}

/// Keep `window` out of screenshots, so the overlay never lands in the capture. Best effort:
/// callers also wait for a hide to land before capturing.
pub fn exclude_from_capture(window: &impl HasWindowHandle) -> Result<(), String> {
    use raw_window_handle::RawWindowHandle;
    let handle = window.window_handle().map_err(|e| e.to_string())?.as_raw();
    match handle {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => {
            use objc2_app_kit::{NSView, NSWindowSharingType};
            // SAFETY: the handle is the live NSView of a gpui window, and this runs on the main thread.
            let view = unsafe { h.ns_view.cast::<NSView>().as_ref() };
            let ns = view.window().ok_or("window has no NSWindow yet")?;
            ns.setSharingType(NSWindowSharingType::None);
            Ok(())
        }
        #[cfg(windows)]
        RawWindowHandle::Win32(h) => {
            use windows::Win32::{
                Foundation::HWND,
                UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE},
            };
            unsafe { SetWindowDisplayAffinity(HWND(h.hwnd.get() as _), WDA_EXCLUDEFROMCAPTURE) }
                .map_err(|e| e.to_string())
        }
        _ => Err("unsupported window handle".into()),
    }
}

/// Let mouse events fall through `window` to whatever is below it.
pub fn click_through(window: &impl HasWindowHandle) -> Result<(), String> {
    use raw_window_handle::RawWindowHandle;
    let handle = window.window_handle().map_err(|e| e.to_string())?.as_raw();
    match handle {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => {
            use objc2_app_kit::NSView;
            // SAFETY: the handle is the live NSView of a gpui window, and this runs on the main thread.
            let view = unsafe { h.ns_view.cast::<NSView>().as_ref() };
            view.window()
                .ok_or("window has no NSWindow yet")?
                .setIgnoresMouseEvents(true);
            Ok(())
        }
        #[cfg(windows)]
        RawWindowHandle::Win32(h) => {
            use windows::Win32::{
                Foundation::{COLORREF, GetLastError, HWND, SetLastError, WIN32_ERROR},
                UI::WindowsAndMessaging::{
                    GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes,
                    SetWindowLongPtrW, WS_EX_LAYERED, WS_EX_TRANSPARENT,
                },
            };
            let hwnd = HWND(h.hwnd.get() as _);
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            // SetWindowLongPtrW returns the previous value, which can legitimately be 0: clear the last error to tell.
            unsafe { SetLastError(WIN32_ERROR(0)) };
            let prev = unsafe {
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    style | (WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0) as isize,
                )
            };
            if prev == 0 {
                let e = unsafe { GetLastError() };
                if e != WIN32_ERROR(0) {
                    return Err(format!("SetWindowLongPtrW failed: {e:?}"));
                }
            }
            // UNVERIFIED on Windows: a layered window is not shown until its attributes are set; full alpha keeps it
            // opaque. Needs a check on a real machine that the DirectComposition content still renders and clicks pass through.
            unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) }
                .map_err(|e| format!("SetLayeredWindowAttributes failed: {e}"))
        }
        _ => Err("unsupported window handle".into()),
    }
}

/// Make `window` cover its whole screen (or `region`, global top-left points), menu bar included. macOS constrains a titled window below the
/// menu bar, which shifts the selector down and misaligns every capture. A borderless window is not
/// constrained. The change runs on the main queue, after the current gpui update: AppKit calls gpui's resize
/// and move handlers synchronously, and they must not re-enter a running update.
pub fn cover_screen(
    window: &impl HasWindowHandle,
    region: Option<(f64, f64, f64, f64)>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSView, NSWindow, NSWindowStyleMask};
        use raw_window_handle::RawWindowHandle;
        struct OnMain(Retained<NSWindow>);
        // SAFETY: only touched on the main queue (the main thread), where AppKit objects may be used.
        unsafe impl Send for OnMain {}
        impl OnMain {
            // A method, so the closure captures the whole wrapper (not its non-Send field).
            fn get(self) -> Retained<NSWindow> {
                self.0
            }
        }

        let RawWindowHandle::AppKit(h) =
            window.window_handle().map_err(|e| e.to_string())?.as_raw()
        else {
            return Err("unsupported window handle".into());
        };
        // SAFETY: the handle is the live NSView of a gpui window, and this runs on the main thread.
        let view = unsafe { h.ns_view.cast::<NSView>().as_ref() };
        let ns = OnMain(view.window().ok_or("window has no NSWindow yet")?);
        dispatch2::DispatchQueue::main().exec_async(move || {
            let ns = ns.get();
            if let Some(screen) = ns.screen() {
                ns.setStyleMask(
                    NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                );
                let mut frame = screen.frame();
                if let Some((x, y, w, h)) = region
                    && let Some(mtm) = objc2::MainThreadMarker::new()
                    && let Some(primary) = objc2_app_kit::NSScreen::screens(mtm).firstObject()
                {
                    // Global top-left points to Cocoa's bottom-left origin on the primary screen.
                    frame.origin.x = x;
                    frame.origin.y = primary.frame().size.height - y - h;
                    frame.size.width = w;
                    frame.size.height = h;
                }
                ns.setFrame_display(frame, true);
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, region);
    Ok(())
}
