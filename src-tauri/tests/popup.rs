use screen_translate_lib::capture::LogicalRect;
use screen_translate_lib::popup::place;

fn lr(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
    LogicalRect { x, y, width, height }
}

const M: f64 = 8.0;

fn screen() -> LogicalRect {
    lr(0.0, 0.0, 1920.0, 1080.0)
}

#[test]
fn room_below_places_below_at_sel_x() {
    assert_eq!(place(lr(100.0, 100.0, 200.0, 50.0), 300.0, 100.0, screen(), M), (100.0, 158.0));
}

#[test]
fn near_bottom_places_above() {
    assert_eq!(place(lr(100.0, 1000.0, 200.0, 50.0), 300.0, 100.0, screen(), M), (100.0, 892.0));
}

#[test]
fn tall_selection_filling_screen_pins_to_screen_bottom() {
    assert_eq!(place(lr(100.0, 0.0, 200.0, 1080.0), 300.0, 100.0, screen(), M), (100.0, 972.0));
}

#[test]
fn near_right_edge_clamps_x() {
    assert_eq!(place(lr(1800.0, 100.0, 100.0, 50.0), 300.0, 100.0, screen(), M), (1612.0, 158.0));
}

#[test]
fn near_left_edge_clamps_x_to_margin() {
    assert_eq!(place(lr(2.0, 100.0, 100.0, 50.0), 300.0, 100.0, screen(), M), (8.0, 158.0));
}

#[test]
fn second_monitor_with_positive_origin() {
    let s = lr(1920.0, 0.0, 1920.0, 1080.0);
    assert_eq!(place(lr(2000.0, 100.0, 200.0, 50.0), 300.0, 100.0, s, M), (2000.0, 158.0));
    assert_eq!(place(lr(2000.0, 1000.0, 200.0, 50.0), 300.0, 100.0, s, M), (2000.0, 892.0));
    assert_eq!(place(lr(1925.0, 100.0, 100.0, 50.0), 300.0, 100.0, s, M), (1928.0, 158.0));
    assert_eq!(place(lr(3800.0, 100.0, 100.0, 50.0), 300.0, 100.0, s, M), (3532.0, 158.0));
}

#[test]
fn second_monitor_with_negative_origin() {
    let s = lr(-1440.0, 0.0, 1440.0, 900.0);
    assert_eq!(place(lr(-1000.0, 100.0, 200.0, 50.0), 300.0, 100.0, s, M), (-1000.0, 158.0));
    assert_eq!(place(lr(-1000.0, 820.0, 200.0, 50.0), 300.0, 100.0, s, M), (-1000.0, 712.0));
    assert_eq!(place(lr(-1438.0, 100.0, 100.0, 50.0), 300.0, 100.0, s, M), (-1432.0, 158.0));
    assert_eq!(place(lr(-100.0, 100.0, 50.0, 50.0), 300.0, 100.0, s, M), (-308.0, 158.0));
}

#[test]
fn nonzero_screen_y_origin() {
    let s = lr(0.0, 200.0, 1920.0, 1080.0);
    assert_eq!(place(lr(100.0, 300.0, 200.0, 50.0), 300.0, 100.0, s, M), (100.0, 358.0));
    assert_eq!(place(lr(100.0, 200.0, 200.0, 1080.0), 300.0, 100.0, s, M), (100.0, 1172.0));
}

#[test]
fn popup_wider_than_screen_aligns_to_left_margin() {
    assert_eq!(place(lr(500.0, 100.0, 200.0, 50.0), 2000.0, 100.0, screen(), M), (8.0, 158.0));
    let s = lr(1920.0, 0.0, 1920.0, 1080.0);
    assert_eq!(place(lr(2500.0, 100.0, 200.0, 50.0), 2000.0, 100.0, s, M), (1928.0, 158.0));
}

#[test]
fn popup_taller_than_screen_aligns_to_top_margin() {
    assert_eq!(place(lr(100.0, 100.0, 200.0, 50.0), 300.0, 1200.0, screen(), M).1, 8.0);
}
