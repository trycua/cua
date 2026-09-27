use super::{megapixel_long_edge_cap, WINDOW_SCREENSHOT_MAX_PIXELS};

#[test]
fn a_window_shot_above_the_api_threshold_is_capped_below_it() {
    let edge = megapixel_long_edge_cap(1568, 861, WINDOW_SCREENSHOT_MAX_PIXELS).expect("capped");
    let scale = edge as f64 / 1568.0;
    let (w, h) = (
        (1568.0 * scale).round() as u64,
        (861.0 * scale).round() as u64,
    );
    assert!(w * h <= WINDOW_SCREENSHOT_MAX_PIXELS, "{w}x{h}");
    assert!(edge >= 1440 && edge < 1568, "{edge}");
}

#[test]
fn a_shot_within_the_threshold_is_left_alone() {
    assert_eq!(
        megapixel_long_edge_cap(1280, 800, WINDOW_SCREENSHOT_MAX_PIXELS),
        None
    );
    assert_eq!(
        megapixel_long_edge_cap(0, 800, WINDOW_SCREENSHOT_MAX_PIXELS),
        None
    );
}
