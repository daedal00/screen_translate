use image::{Rgba, RgbaImage};
use screen_translate_core::capture::{LogicalRect, PhysicalRect, crop, pick_monitor, to_physical};

fn lr(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
    LogicalRect {
        x,
        y,
        width,
        height,
    }
}

fn pr(x: u32, y: u32, width: u32, height: u32) -> PhysicalRect {
    PhysicalRect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn scale_1_is_identity() {
    assert_eq!(
        to_physical(lr(10.0, 20.0, 100.0, 50.0), 1.0, 1920, 1080),
        Some(pr(10, 20, 100, 50))
    );
}

#[test]
fn scale_1_25_rounds_outward() {
    // x: 12.5 -> 12, right: 137.5 -> 138, y: 12.5 -> 12, bottom: 75 -> 75
    assert_eq!(
        to_physical(lr(10.0, 10.0, 100.0, 50.0), 1.25, 1920, 1080),
        Some(pr(12, 12, 126, 63))
    );
}

#[test]
fn scale_1_5_floors_origin_and_ceils_far_edge() {
    // x: 15.45 -> 15, right: 45.45 -> 46, y: 30.3 -> 30, bottom: 45.3 -> 46
    assert_eq!(
        to_physical(lr(10.3, 20.2, 20.0, 10.0), 1.5, 1920, 1080),
        Some(pr(15, 30, 31, 16))
    );
}

#[test]
fn scale_2_retina() {
    assert_eq!(
        to_physical(lr(5.0, 7.0, 10.0, 20.0), 2.0, 2880, 1800),
        Some(pr(10, 14, 20, 40))
    );
}

#[test]
fn negative_size_is_normalized() {
    // dragged up-left from (50, 40)
    assert_eq!(
        to_physical(lr(50.0, 40.0, -20.0, -10.0), 1.0, 1920, 1080),
        Some(pr(30, 30, 20, 10))
    );
}

#[test]
fn negative_size_is_normalized_with_scale() {
    assert_eq!(
        to_physical(lr(50.0, 40.0, -20.0, -10.0), 2.0, 1920, 1080),
        Some(pr(60, 60, 40, 20))
    );
}

#[test]
fn clamped_past_right_and_bottom() {
    assert_eq!(
        to_physical(lr(90.0, 90.0, 50.0, 50.0), 1.0, 100, 100),
        Some(pr(90, 90, 10, 10))
    );
}

#[test]
fn clamped_when_starting_at_negative_coords() {
    assert_eq!(
        to_physical(lr(-10.0, -5.0, 30.0, 20.0), 1.0, 100, 100),
        Some(pr(0, 0, 20, 15))
    );
}

#[test]
fn clamped_after_scaling() {
    // 45*2 = 90, right edge 60*2 = 120 -> clamped to 100
    assert_eq!(
        to_physical(lr(45.0, 45.0, 15.0, 15.0), 2.0, 100, 100),
        Some(pr(90, 90, 10, 10))
    );
}

#[test]
fn none_when_fully_outside() {
    assert_eq!(
        to_physical(lr(200.0, 200.0, 10.0, 10.0), 1.0, 100, 100),
        None
    );
    assert_eq!(
        to_physical(lr(-50.0, 10.0, 20.0, 10.0), 1.0, 100, 100),
        None
    );
    assert_eq!(
        to_physical(lr(10.0, -50.0, 20.0, 10.0), 1.0, 100, 100),
        None
    );
}

#[test]
fn none_when_zero_size() {
    assert_eq!(to_physical(lr(10.0, 10.0, 0.0, 10.0), 1.0, 100, 100), None);
    assert_eq!(to_physical(lr(10.0, 10.0, 10.0, 0.0), 2.0, 100, 100), None);
}

fn gradient(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| Rgba([x as u8, y as u8, 0, 255]))
}

#[test]
fn crop_has_correct_dimensions() {
    let out = crop(&gradient(10, 8), pr(2, 3, 5, 4));
    assert_eq!(out.dimensions(), (5, 4));
}

#[test]
fn crop_has_correct_pixels() {
    let out = crop(&gradient(10, 8), pr(2, 3, 5, 4));
    assert_eq!(*out.get_pixel(0, 0), Rgba([2, 3, 0, 255]));
    assert_eq!(*out.get_pixel(4, 3), Rgba([6, 6, 0, 255]));
    assert_eq!(*out.get_pixel(4, 0), Rgba([6, 3, 0, 255]));
}

#[test]
fn crop_full_image_is_identical() {
    let img = gradient(6, 4);
    assert_eq!(crop(&img, pr(0, 0, 6, 4)), img);
}

#[test]
fn to_physical_then_crop_end_to_end() {
    let img = gradient(20, 20);
    let rect = to_physical(lr(2.0, 3.0, 4.0, 5.0), 2.0, 20, 20).unwrap();
    let out = crop(&img, rect);
    assert_eq!(out.dimensions(), (8, 10));
    assert_eq!(*out.get_pixel(0, 0), Rgba([4, 6, 0, 255]));
}

#[test]
fn fractional_scale_with_negative_origin_rect() {
    // x,y: -0.45 -> clamped to 0; right/bottom: (-0.3 + 10) * 1.5 = 14.55 -> ceil 15
    assert_eq!(
        to_physical(lr(-0.3, -0.3, 10.0, 10.0), 1.5, 100, 100),
        Some(pr(0, 0, 15, 15))
    );
}

#[test]
fn nan_input_returns_none() {
    assert_eq!(
        to_physical(lr(f64::NAN, 0.0, 10.0, 10.0), 1.0, 100, 100),
        None
    );
    assert_eq!(
        to_physical(lr(0.0, 0.0, f64::NAN, 10.0), 1.0, 100, 100),
        None
    );
    assert_eq!(
        to_physical(lr(0.0, 0.0, 10.0, 10.0), f64::NAN, 100, 100),
        None
    );
}

// MARKER_PICK
// A 1920@1x at x=0, B 1920@1x at x=1920, C 4K@2x at x=3840 (physical origins).
// Logical origin of C is (1920, 0), which collides with B; matching must use one unit system only.
#[test]
fn pick_monitor_three_monitor_layout() {
    let origins = [(0, 0), (1920, 0), (3840, 0)];
    assert_eq!(pick_monitor(&origins, (3840, 0)), Some(2));
    assert_eq!(pick_monitor(&origins, (1920, 0)), Some(1));
    assert_eq!(pick_monitor(&origins, (0, 0)), Some(0));
}

#[test]
fn pick_monitor_negative_origin() {
    let origins = [(-2560, 0), (0, 0)];
    assert_eq!(pick_monitor(&origins, (-2560, 0)), Some(0));
    assert_eq!(pick_monitor(&origins, (0, 0)), Some(1));
}

#[test]
fn pick_monitor_no_match() {
    assert_eq!(pick_monitor(&[(0, 0), (1920, 0)], (1920, 5)), None);
}

#[test]
fn pick_monitor_empty() {
    assert_eq!(pick_monitor(&[], (0, 0)), None);
}

/// True when at least `threshold` (0..1) of pixels have r,g,b all <= 16.
fn mostly_black(img: &RgbaImage, threshold: f64) -> bool {
    let total = (img.width() as u64) * (img.height() as u64);
    if total == 0 {
        return true;
    }
    let dark = img
        .pixels()
        .filter(|p| p.0[..3].iter().all(|&c| c <= 16))
        .count() as u64;
    dark as f64 / total as f64 >= threshold
}

#[test]
fn mostly_black_detects_black_and_mixed() {
    let black = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
    assert!(mostly_black(&black, 0.95));
    let mut mixed = black.clone();
    for x in 0..10 {
        for y in 0..5 {
            mixed.put_pixel(x, y, Rgba([200, 180, 90, 255]));
        }
    }
    assert!(!mostly_black(&mixed, 0.95));
    assert!(mostly_black(&RgbaImage::new(0, 0), 0.95));
}

/// Manual repro: CAPTURE_RECT="x,y,w,h" (points, main display local) over a playing video.
/// Optional CAPTURE_OUT=path.png saves the captured image.
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn video_region_is_not_black() {
    let raw = std::env::var("CAPTURE_RECT").expect("set CAPTURE_RECT=x,y,w,h");
    let v: Vec<f64> = raw.split(',').map(|s| s.trim().parse().unwrap()).collect();
    let rect = lr(v[0], v[1], v[2], v[3]);
    let img = screen_translate_core::capture::grab_region((0, 0), rect, 1.0)
        .expect("capture failed")
        .expect("empty capture rect");
    if let Ok(out) = std::env::var("CAPTURE_OUT") {
        img.save(&out).unwrap();
    }
    assert!(img.width() > 0 && img.height() > 0, "empty image");
    assert!(
        !mostly_black(&img, 0.95),
        "captured video region is almost entirely black ({}x{})",
        img.width(),
        img.height()
    );
}
