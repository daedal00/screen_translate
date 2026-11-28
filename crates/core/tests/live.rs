use image::{Rgba, RgbaImage};
use screen_translate_core::capture::PhysicalRect;
use screen_translate_core::live::{
    ChangeGate, Fingerprint, OverlayLine, distance, fingerprint, layout,
};

fn split(w: u32, h: u32, left: u8, right: u8) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, _| {
        let v = if x < w / 2 { left } else { right };
        Rgba([v, v, v, 255])
    })
}

fn rect(height: u32) -> PhysicalRect {
    PhysicalRect {
        x: 0,
        y: 0,
        width: 100,
        height,
    }
}

/// 64x64 image where the first `n` cells (row-major, 2x2 px each) of the 32x32 grid are white,
/// the rest black. `distance(cells(a), cells(b)) == |a - b|`.
fn cells(n: u32) -> Fingerprint {
    fingerprint(&RgbaImage::from_fn(64, 64, |x, y| {
        let v = if (y / 2) * 32 + x / 2 < n { 255 } else { 0 };
        Rgba([v, v, v, 255])
    }))
}

/// White 600x80 image with a dark "text line" of pseudo-random glyph strokes.
fn text_line(seed: u64) -> RgbaImage {
    let mut s = seed;
    let mut next = |n: u32| {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 33) % n as u64) as u32
    };
    let mut img = RgbaImage::from_pixel(600, 80, Rgba([255, 255, 255, 255]));
    for g in 0..40 {
        for _ in 0..3 {
            let (x, y, w, h) = (
                10 + g * 14 + next(8),
                30 + next(14),
                1 + next(4),
                2 + next(5),
            );
            for yy in y..y + h {
                for xx in x..x + w {
                    img.put_pixel(xx, yy, Rgba([0, 0, 0, 255]));
                }
            }
        }
    }
    img
}

// fingerprint / distance

#[test]
fn distance_counts_differing_cells() {
    assert_eq!(distance(&cells(0), &cells(0)), 0);
    assert_eq!(distance(&cells(0), &cells(1024)), 1024);
    assert_eq!(distance(&cells(10), &cells(3)), 7);
    assert_eq!(distance(&cells(3), &cells(10)), 7);
}

#[test]
fn fingerprint_identical_images_match() {
    let a = split(200, 100, 20, 230);
    assert_eq!(fingerprint(&a), fingerprint(&a.clone()));
    assert_eq!(distance(&fingerprint(&a), &fingerprint(&a)), 0);
}

#[test]
fn fingerprint_flipped_halves_are_far_apart() {
    let a = fingerprint(&split(200, 100, 20, 230));
    let b = fingerprint(&split(200, 100, 230, 20));
    assert!(distance(&a, &b) >= 500, "distance was {}", distance(&a, &b));
}

#[test]
fn fingerprint_one_pixel_change_is_nearly_identical() {
    let a = split(200, 100, 20, 230);
    let mut b = a.clone();
    b.put_pixel(50, 50, Rgba([255, 255, 255, 255]));
    assert!(distance(&fingerprint(&a), &fingerprint(&b)) <= 1);
}

#[test]
fn fingerprint_text_swap_in_same_layout_is_detected() {
    let a = fingerprint(&text_line(1));
    assert_eq!(distance(&a, &fingerprint(&text_line(1))), 0);
    let d = distance(&a, &fingerprint(&text_line(2)));
    assert!(d > 8, "text swap distance was {d}");
}

// ChangeGate

#[test]
fn gate_first_frame_triggers_after_settle_ticks() {
    let mut g = ChangeGate::new(2, 1);
    assert!(!g.tick(cells(0)));
    assert!(g.tick(cells(0)));
}

#[test]
fn gate_settle_two_needs_two_stable_ticks() {
    let mut g = ChangeGate::new(2, 2);
    assert!(!g.tick(cells(0)));
    assert!(!g.tick(cells(0)));
    assert!(g.tick(cells(0)));
}

#[test]
fn gate_unchanged_frames_never_retrigger() {
    let mut g = ChangeGate::new(2, 1);
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    for _ in 0..10 {
        assert!(!g.tick(cells(0)));
    }
}

#[test]
fn gate_change_triggers_only_after_settling() {
    let mut g = ChangeGate::new(2, 2);
    g.tick(cells(0));
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    assert!(!g.tick(cells(1024)));
    assert!(!g.tick(cells(1024)));
    assert!(g.tick(cells(1024)));
    assert!(!g.tick(cells(1024)));
}

#[test]
fn gate_change_with_settle_one() {
    let mut g = ChangeGate::new(2, 1);
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    assert!(!g.tick(cells(1024)));
    assert!(g.tick(cells(1024)));
}

#[test]
fn gate_continuous_animation_never_triggers() {
    let mut g = ChangeGate::new(2, 1);
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    for i in 0..20 {
        let fp = if i % 2 == 0 { cells(1024) } else { cells(0) };
        assert!(!g.tick(fp), "tick {i} triggered");
    }
}

#[test]
fn gate_animation_from_start_never_triggers() {
    let mut g = ChangeGate::new(2, 1);
    for i in 0..20 {
        let fp = if i % 2 == 0 { cells(0) } else { cells(1024) };
        assert!(!g.tick(fp), "tick {i} triggered");
    }
}

#[test]
fn gate_noise_under_threshold_does_not_trigger() {
    let mut g = ChangeGate::new(2, 1);
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    for n in [1, 2, 1, 1, 0] {
        assert!(!g.tick(cells(n)));
    }
}

#[test]
fn gate_settles_again_after_animation_ends() {
    let mut g = ChangeGate::new(2, 1);
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    assert!(!g.tick(cells(1024)));
    assert!(!g.tick(cells(512)));
    assert!(g.tick(cells(512)));
}

#[test]
fn gate_slow_drift_does_not_fire_until_it_stops() {
    // Each step is within threshold (8) of the previous frame, but the cumulative drift is far past it.
    let mut g = ChangeGate::new(8, 2);
    g.tick(cells(0));
    g.tick(cells(0));
    assert!(g.tick(cells(0)));
    for n in (5..=50).step_by(5) {
        assert!(!g.tick(cells(n)), "fired mid-drift at {n}");
    }
    assert!(!g.tick(cells(50)));
    assert!(g.tick(cells(50)), "should fire once after settling");
    assert!(!g.tick(cells(50)));
}

// layout

#[test]
fn layout_scale_one_keeps_coordinates() {
    let r = PhysicalRect {
        x: 10,
        y: 20,
        width: 100,
        height: 24,
    };
    let out = layout(&[(r, "hola".into())], 1.0);
    assert_eq!(
        out,
        vec![OverlayLine {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 24.0,
            text: "hola".into(),
            font_px: 18.0
        }]
    );
}

#[test]
fn layout_scale_two_halves_coordinates() {
    let r = PhysicalRect {
        x: 20,
        y: 40,
        width: 200,
        height: 48,
    };
    let out = layout(&[(r, "hi".into())], 2.0);
    assert_eq!(out.len(), 1);
    let l = &out[0];
    assert_eq!((l.x, l.y, l.width, l.height), (10.0, 20.0, 100.0, 24.0));
    assert_eq!(l.font_px, 18.0);
}

#[test]
fn layout_font_clamps_low_and_high() {
    let out = layout(&[(rect(4), "a".into()), (rect(200), "b".into())], 1.0);
    assert_eq!(out[0].font_px, 10.0);
    assert_eq!(out[1].font_px, 48.0);
}

#[test]
fn layout_drops_empty_translations_and_keeps_order() {
    let lines = vec![
        (rect(20), "one".to_string()),
        (rect(20), String::new()),
        (rect(30), "three".to_string()),
    ];
    let out = layout(&lines, 1.0);
    let texts: Vec<_> = out.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["one", "three"]);
}

#[test]
fn layout_empty_input() {
    assert!(layout(&[], 2.0).is_empty());
}

// edge cases

#[test]
fn fingerprint_tiny_and_odd_sizes_do_not_panic_and_are_deterministic() {
    for (w, h) in [
        (1, 1),
        (1, 9),
        (9, 1),
        (3, 5),
        (7, 7),
        (9, 9),
        (13, 17),
        (31, 31),
        (0, 0),
    ] {
        let img = split(w, h, 10, 240);
        assert_eq!(fingerprint(&img), fingerprint(&img.clone()), "{w}x{h}");
    }
}

#[test]
fn fingerprint_non_multiple_of_grid_still_separates_halves() {
    let d = distance(
        &fingerprint(&split(37, 21, 0, 255)),
        &fingerprint(&split(37, 21, 255, 0)),
    );
    assert!(d >= 400, "distance {d}");
}

#[test]
fn gate_settle_zero_fires_on_first_frame_then_on_each_real_change() {
    let mut g = ChangeGate::new(2, 0);
    assert!(g.tick(cells(0)));
    assert!(!g.tick(cells(0)));
    assert!(g.tick(cells(1024)));
    assert!(!g.tick(cells(1024)));
}
