use screen_translate_app::overlay::{popup_bounds, selector_bounds};
use screen_translate_core::capture::LogicalRect;
use screen_translate_core::pipeline::Target;

fn target(pos: (i32, i32)) -> Target {
    Target {
        scale: 2.0,
        pos,
        size: (3000, 2000),
        origin: (pos.0 / 2, pos.1 / 2),
        display_id: 7,
    }
}

const SEL: LogicalRect = LogicalRect {
    x: 100.0,
    y: 100.0,
    width: 200.0,
    height: 50.0,
};

#[test]
fn display_local_bounds_ignore_the_monitor_position() {
    let (a, b) = (target((0, 0)), target((3024, 0)));
    assert_eq!(
        selector_bounds(&a, false),
        LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 1500.0,
            height: 1000.0
        }
    );
    assert_eq!(selector_bounds(&a, false), selector_bounds(&b, false));
    assert_eq!(popup_bounds(&a, SEL, false), popup_bounds(&b, SEL, false));
}

#[test]
fn global_bounds_add_the_logical_monitor_origin() {
    let t = target((3000, 200));
    assert_eq!(
        selector_bounds(&t, true),
        LogicalRect {
            x: 1500.0,
            y: 100.0,
            width: 1500.0,
            height: 1000.0
        }
    );
    let local = popup_bounds(&t, SEL, false);
    assert_eq!(
        popup_bounds(&t, SEL, true),
        LogicalRect {
            x: local.x + 1500.0,
            y: local.y + 100.0,
            ..local
        }
    );
}
