//! Milestone 3a RED: popup view state. Needs `screen_translate_app::popup`.
//! Implementer notes: the Close / Settings / Copy buttons must call pub methods `close`, `open_settings`,
//! `copy` (each `(&mut self, cx: &mut Context<Self>)`) that emit the events; the tests call those methods
//! since button pixel positions are layout-dependent. `PopupEvent` derives Clone + Debug + PartialEq.
//! Escape must reach the view without the test focusing anything (window.on_key_event or lazy focus in render).
use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, TestAppContext, VisualTestContext};
use screen_translate_app::popup::{PopupEvent, PopupView};
use screen_translate_core::pipeline::RegionResult;

fn setup(
    cx: &mut TestAppContext,
) -> (
    Entity<PopupView>,
    &mut VisualTestContext,
    Rc<RefCell<Vec<PopupEvent>>>,
) {
    let (view, cx) = cx.add_window_view(|_, _| PopupView::new());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &PopupEvent, _| {
            sink.borrow_mut().push(e.clone())
        })
        .detach();
    });
    cx.run_until_parked();
    (view, cx, events)
}

fn ok(original: &str, translation: &str, src: Option<&str>) -> RegionResult {
    RegionResult {
        original: original.into(),
        translation: Some(translation.into()),
        detected_source: src.map(Into::into),
        error: None,
        needs_settings: false,
    }
}

#[gpui::test]
fn pending_state(cx: &mut TestAppContext) {
    let (view, cx, _) = setup(cx);
    view.update(cx, |v, _| v.pending());
    view.read_with(cx, |v, _| {
        assert!(v.is_pending());
        assert_eq!(
            v.translation_text(),
            Some("Translating\u{2026}".to_string())
        );
        assert_eq!(v.error_text(), None);
        assert_eq!(v.original_text(), "");
        assert_eq!(v.source_label(), "");
        assert!(!v.copy_visible());
        assert!(!v.settings_visible());
        assert_eq!(v.copy_label(), "Copy");
    });
}

#[gpui::test]
fn show_success(cx: &mut TestAppContext) {
    let (view, cx, _) = setup(cx);
    view.update(cx, |v, _| {
        v.pending();
        v.show(ok("Guten Tag", "Good day", Some("DE")));
    });
    view.read_with(cx, |v, _| {
        assert!(!v.is_pending());
        assert_eq!(v.translation_text(), Some("Good day".to_string()));
        assert_eq!(v.error_text(), None);
        assert_eq!(v.original_text(), "Guten Tag");
        assert_eq!(v.source_label(), "From DE");
        assert!(v.copy_visible());
        assert!(!v.settings_visible());
    });
}

#[gpui::test]
fn show_without_detected_source_has_empty_label(cx: &mut TestAppContext) {
    let (view, cx, _) = setup(cx);
    view.update(cx, |v, _| v.show(ok("hola", "hello", None)));
    assert_eq!(view.read_with(cx, |v, _| v.source_label()), "");
}

#[gpui::test]
fn show_error(cx: &mut TestAppContext) {
    let (view, cx, _) = setup(cx);
    view.update(cx, |v, _| {
        v.show(RegionResult::failed("text".into(), "boom"))
    });
    view.read_with(cx, |v, _| {
        assert!(!v.is_pending());
        assert_eq!(v.translation_text(), None);
        assert_eq!(v.error_text(), Some("boom".to_string()));
        assert_eq!(v.original_text(), "text");
        assert!(!v.copy_visible());
        assert!(!v.settings_visible());
    });
}

#[gpui::test]
fn show_needs_settings(cx: &mut TestAppContext) {
    let (view, cx, _) = setup(cx);
    let mut r = RegionResult::failed("text".into(), "No API key");
    r.needs_settings = true;
    view.update(cx, |v, _| v.show(r));
    view.read_with(cx, |v, _| {
        assert!(v.settings_visible());
        assert_eq!(v.error_text(), Some("No API key".to_string()));
        assert!(!v.copy_visible());
    });
}

#[gpui::test]
fn show_after_error_clears_error(cx: &mut TestAppContext) {
    let (view, cx, _) = setup(cx);
    view.update(cx, |v, _| {
        v.show(RegionResult::failed("a".into(), "boom"));
        v.show(ok("b", "c", Some("FR")));
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.error_text(), None);
        assert_eq!(v.translation_text(), Some("c".to_string()));
    });
}

#[gpui::test]
fn copy_emits_translation_and_label_lifecycle(cx: &mut TestAppContext) {
    let (view, cx, events) = setup(cx);
    view.update(cx, |v, _| v.show(ok("Guten Tag", "Good day", Some("DE"))));
    assert_eq!(
        view.read_with(cx, |v, _| v.copy_label().to_string()),
        "Copy"
    );
    view.update(cx, |v, cx| v.copy(cx));
    assert_eq!(
        *events.borrow(),
        vec![PopupEvent::Copy("Good day".to_string())]
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.copy_label().to_string()),
        "Copied"
    );
    view.update(cx, |v, _| v.pending());
    assert_eq!(
        view.read_with(cx, |v, _| v.copy_label().to_string()),
        "Copy"
    );
}

#[gpui::test]
fn close_and_settings_buttons_emit(cx: &mut TestAppContext) {
    let (view, cx, events) = setup(cx);
    view.update(cx, |v, cx| {
        v.close(cx);
        v.open_settings(cx);
    });
    assert_eq!(
        *events.borrow(),
        vec![PopupEvent::Close, PopupEvent::OpenSettings]
    );
}

#[gpui::test]
fn escape_closes(cx: &mut TestAppContext) {
    let (_view, cx, events) = setup(cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(*events.borrow(), vec![PopupEvent::Close]);
}
