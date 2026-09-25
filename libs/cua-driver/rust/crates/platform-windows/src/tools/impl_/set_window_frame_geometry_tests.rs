
use super::outer_frame_for_visible_request;

#[test]
fn compensates_for_invisible_resize_borders() {
    assert_eq!(
            outer_frame_for_visible_request(
                (65, 52, 872, 626),
                (40, 40, 886, 633),
                (47, 40, 872, 626),
            )
            .unwrap(),
            (58, 52, 886, 633)
        );
}

#[test]
fn leaves_outer_request_unchanged_when_dwm_bounds_are_unavailable() {
    assert_eq!(
        outer_frame_for_visible_request(
            (-25, 10, 640, 480),
            (20, 30, 800, 600),
            (20, 30, 800, 600),
        )
        .unwrap(),
        (-25, 10, 640, 480)
    );
}
