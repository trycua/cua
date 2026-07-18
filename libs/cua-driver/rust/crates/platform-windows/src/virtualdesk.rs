//! Pure math for `MOUSEEVENTF_VIRTUALDESK` absolute-coordinate normalization.
//!
//! `SendInput` with `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` interprets
//! `MOUSEINPUT::dx`/`dy` as **normalized** coordinates in `0..=65535` spanning
//! the union of all monitors (the "virtual desktop"). `(0, 0)` is the upper-left
//! of the virtual screen and `(65535, 65535)` is the lower-right.
//!
//! In multi-monitor layouts where a secondary monitor sits to the **left** of
//! (or **above**) the primary, `GetSystemMetrics(SM_XVIRTUALSCREEN)` /
//! `SM_YVIRTUALSCREEN` are **negative** (the primary's top-left stays at
//! `(0, 0)` and the virtual rect extends into negative space). The forward
//! mapping must therefore translate by the virtual origin BEFORE the scale, and
//! the result must land within the `0..=65535` band — issue #1979 reports
//! clicks misrouting on exactly these layouts, so the math lives in a pure
//! function we can unit-test on any host (no Windows runtime needed).
//!
//! Round-trip target: feeding `to_virtualdesk_absolute` followed by the inverse
//! `from_virtualdesk_absolute` must recover the input screen pixel within ±1
//! (rounding only — never sign-flip, never land on a different monitor).

/// Convert a screen-space pixel `(sx, sy)` to the `MOUSEEVENTF_VIRTUALDESK`
/// normalized absolute coordinate `(dx, dy)` for `MOUSEINPUT`.
///
/// Inputs:
/// - `sx`, `sy`: screen pixel, in the virtual-desktop coordinate system
///   (may be negative if the click target is on a monitor to the left of /
///   above the primary).
/// - `virt_x`, `virt_y`: `GetSystemMetrics(SM_XVIRTUALSCREEN | SM_YVIRTUALSCREEN)`.
///   Negative when a secondary monitor extends past the primary's origin.
/// - `virt_w`, `virt_h`: `GetSystemMetrics(SM_CXVIRTUALSCREEN | SM_CYVIRTUALSCREEN)`.
///   Always positive; the call sites clamp `.max(1)` to avoid divide-by-zero
///   on (impossible-in-practice) 0-width virtual desks.
///
/// Returns a value in `0..=65535` (clamped) that, when reverse-mapped by
/// Windows using the same virtual-desktop rect, recovers `(sx, sy)` within ±1
/// pixel of rounding.
///
/// All intermediate math is i64 so that products like
/// `(sx - virt_x) * 65535` cannot overflow on any realistic display
/// configuration (10k px wide × 65535 still fits comfortably).
///
/// Reasoning for the `(virt_w - 1)` divisor (vs the more obvious `virt_w`):
/// Windows maps `dx = 0` to the **first** column of the virtual screen
/// (`screen_x = virt_x`) and `dx = 65535` to the **last** column
/// (`screen_x = virt_x + virt_w - 1`). The forward map is therefore
/// `dx = (sx - virt_x) * 65535 / (virt_w - 1)`. The off-by-one matters for the
/// rightmost / bottommost pixel of the virtual desktop (dx = 65535 ↔ last
/// pixel, not "one past the last"); negative-offset layouts inherit the same
/// fence-post logic on their left/top edges.
#[inline]
pub fn to_virtualdesk_absolute(
    sx: i32,
    sy: i32,
    virt_x: i32,
    virt_y: i32,
    virt_w: i32,
    virt_h: i32,
) -> (i32, i32) {
    // Divisor floor of 1 — virt_w/virt_h are sourced from GetSystemMetrics with
    // `.max(1)` at the call site, so a zero divisor cannot reach here, but we
    // keep the guard local in case the helper is reused elsewhere.
    let denom_x = (virt_w as i64 - 1).max(1);
    let denom_y = (virt_h as i64 - 1).max(1);
    // i64 arithmetic — `sx - virt_x` stays in i32 range for any realistic
    // monitor layout, and the * 65535 multiply needs i64 headroom.
    let nx = (sx as i64 - virt_x as i64) * 65535 / denom_x;
    let ny = (sy as i64 - virt_y as i64) * 65535 / denom_y;
    (nx.clamp(0, 65535) as i32, ny.clamp(0, 65535) as i32)
}

/// Inverse of [`to_virtualdesk_absolute`] — given an absolute coordinate `(dx, dy)`
/// in `0..=65535` and the virtual-desktop rect, recover the screen pixel.
///
/// This mirrors Windows' own internal reverse mapping (`dx * (virt_w - 1) / 65535
/// + virt_x`) and is used **only** by the round-trip tests; the production
/// click dispatcher never needs to reverse the normalization.
#[inline]
#[cfg(test)]
pub fn from_virtualdesk_absolute(
    dx: i32,
    dy: i32,
    virt_x: i32,
    virt_y: i32,
    virt_w: i32,
    virt_h: i32,
) -> (i32, i32) {
    let denom = 65535_i64;
    // Round-to-nearest for the inverse — the forward direction truncates, so a
    // truncating inverse compounds rounding bias and trips the ±1 round-trip
    // assertion at the right/bottom edges.
    let span_x = (virt_w as i64 - 1).max(0);
    let span_y = (virt_h as i64 - 1).max(0);
    let sx = (dx as i64 * span_x + denom / 2) / denom + virt_x as i64;
    let sy = (dy as i64 * span_y + denom / 2) / denom + virt_y as i64;
    (sx as i32, sy as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layouts we care about (issue #1979 surfaces the LEFT case).
    ///
    /// Each tuple: (label, virt_x, virt_y, virt_w, virt_h, sample screen pts).
    /// The sample points cover: the four corners of the virtual desktop, plus
    /// any case-specific pixel of interest (e.g. the reporter's `(-1795, 383)`).
    fn layouts() -> Vec<(&'static str, i32, i32, i32, i32, Vec<(i32, i32)>)> {
        vec![
            (
                // The reporter's setup: secondary to the LEFT of primary, both 1920x1080.
                // Primary occupies (0..1920, 0..1080); secondary occupies (-1920..0, 0..1080).
                "secondary-left-of-primary",
                -1920,
                0,
                3840,
                1080,
                vec![
                    (-1920, 0),   // top-left of virtual desk (top-left of secondary)
                    (1919, 1079), // bottom-right of virtual desk (bottom-right of primary)
                    (-1, 0),      // last pixel of secondary on the X seam
                    (0, 0),       // first pixel of primary on the X seam
                    (-1795, 383), // the exact pixel from issue #1979
                    (-1198, 292), // the second pixel from issue #1979
                ],
            ),
            (
                // Secondary to the RIGHT of primary. Virt origin at (0, 0), width 3840.
                "secondary-right-of-primary",
                0,
                0,
                3840,
                1080,
                vec![
                    (0, 0),
                    (3839, 1079),
                    (1919, 0),   // last pixel of primary on seam
                    (1920, 0),   // first pixel of secondary on seam
                    (2125, 383), // mirror of the reporter's pixel onto the right layout
                ],
            ),
            (
                // Secondary ABOVE primary. Virt origin at (0, -1080), height 2160.
                "secondary-above-primary",
                0,
                -1080,
                1920,
                2160,
                vec![(0, -1080), (1919, 1079), (0, -1), (0, 0), (960, -540)],
            ),
            (
                // Secondary BELOW primary. Virt origin at (0, 0), height 2160.
                "secondary-below-primary",
                0,
                0,
                1920,
                2160,
                vec![(0, 0), (1919, 2159), (0, 1079), (0, 1080), (960, 1620)],
            ),
            (
                // Three horizontal monitors: [-1920..0, 0..1920, 1920..3840], all 1080 tall.
                // virt rect (-1920, 0, 5760, 1080).
                "three-horiz-mixed-signs",
                -1920,
                0,
                5760,
                1080,
                vec![
                    (-1920, 0),
                    (-1, 0),
                    (0, 0),
                    (1919, 0),
                    (1920, 0),
                    (3839, 1079),
                    (-1795, 383), // reporter's pixel still on leftmost monitor
                ],
            ),
            (
                // Single monitor — the baseline. No offsets.
                "single-monitor-1920x1080",
                0,
                0,
                1920,
                1080,
                vec![(0, 0), (1919, 1079), (960, 540), (1, 1)],
            ),
            (
                // HiDPI secondary. cua-driver runs Per-Monitor-V2 DPI-aware (see
                // win32 manifest), so GetSystemMetrics returns PIXEL extents
                // regardless of DPI scaling — the normalization math is identical
                // to the same-resolution case. We exercise a larger secondary
                // (e.g. 2560x1440) to the left of a 1920x1080 primary to confirm
                // there's no implicit DPI assumption in the helper.
                //
                // virt rect (-2560, 0, 4480, 1440); secondary occupies
                // (-2560..0, 0..1440), primary occupies (0..1920, 0..1080) inside
                // the same virt height band.
                "hidpi-secondary-left-2560x1440",
                -2560,
                0,
                4480,
                1440,
                vec![
                    (-2560, 0),
                    (4479 - 2560, 1439), // bottom-right of virtual desk
                    (-1, 0),
                    (0, 0),
                    (-2000, 720),
                    (1000, 540),
                ],
            ),
            (
                // Pathological tall layout: primary at top, two secondaries stacked
                // below, mixed with one to the right of primary. Just a sanity
                // case for a virt rect spanning both positive and negative.
                "mixed-vertical-and-horizontal",
                -1920,
                -1080,
                3840,
                2160,
                vec![
                    (-1920, -1080),
                    (1919, 1079),
                    (0, 0),
                    (-960, -540),
                    (960, 540),
                ],
            ),
        ]
    }

    #[test]
    fn output_always_in_normalized_range() {
        for (label, vx, vy, vw, vh, points) in layouts() {
            for (sx, sy) in points {
                let (dx, dy) = to_virtualdesk_absolute(sx, sy, vx, vy, vw, vh);
                assert!(
                    (0..=65535).contains(&dx),
                    "{label}: dx={dx} out of [0, 65535] for screen ({sx}, {sy})"
                );
                assert!(
                    (0..=65535).contains(&dy),
                    "{label}: dy={dy} out of [0, 65535] for screen ({sx}, {sy})"
                );
            }
        }
    }

    #[test]
    fn round_trip_recovers_input_within_one_pixel() {
        for (label, vx, vy, vw, vh, points) in layouts() {
            for (sx, sy) in points {
                let (dx, dy) = to_virtualdesk_absolute(sx, sy, vx, vy, vw, vh);
                let (rx, ry) = from_virtualdesk_absolute(dx, dy, vx, vy, vw, vh);
                assert!(
                    (rx - sx).abs() <= 1,
                    "{label}: x round-trip drift: screen={sx} → abs={dx} → screen={rx} (diff {})",
                    rx - sx
                );
                assert!(
                    (ry - sy).abs() <= 1,
                    "{label}: y round-trip drift: screen={sy} → abs={dy} → screen={ry} (diff {})",
                    ry - sy
                );
            }
        }
    }

    #[test]
    fn monotonic_in_x_and_y() {
        // Two adjacent X pixels must produce non-decreasing absolute coords;
        // ditto for Y. If the formula has a sign-flip or `.abs()` bug on
        // negative offsets, monotonicity breaks across the (-1, 0) seam.
        for (label, vx, vy, vw, vh, _) in layouts() {
            for sx in (vx..vx + vw - 1).step_by(7) {
                let (dx_a, _) = to_virtualdesk_absolute(sx, vy, vx, vy, vw, vh);
                let (dx_b, _) = to_virtualdesk_absolute(sx + 1, vy, vx, vy, vw, vh);
                assert!(
                    dx_a <= dx_b,
                    "{label}: x monotonicity break at sx={sx}: {dx_a} > {dx_b}"
                );
            }
            for sy in (vy..vy + vh - 1).step_by(7) {
                let (_, dy_a) = to_virtualdesk_absolute(vx, sy, vx, vy, vw, vh);
                let (_, dy_b) = to_virtualdesk_absolute(vx, sy + 1, vx, vy, vw, vh);
                assert!(
                    dy_a <= dy_b,
                    "{label}: y monotonicity break at sy={sy}: {dy_a} > {dy_b}"
                );
            }
        }
    }

    #[test]
    fn corners_map_to_extreme_normalized_values() {
        // The corners of the virtual desktop must hit the extremes of the
        // normalized range. If a sign error is folding negative coords into
        // the positive band (via `.abs()` for example), the top-left of a
        // negative-origin layout would map to a non-zero dx.
        for (label, vx, vy, vw, vh, _) in layouts() {
            let (tl_x, tl_y) = to_virtualdesk_absolute(vx, vy, vx, vy, vw, vh);
            assert_eq!(tl_x, 0, "{label}: top-left x ≠ 0 (got {tl_x})");
            assert_eq!(tl_y, 0, "{label}: top-left y ≠ 0 (got {tl_y})");
            let (br_x, br_y) = to_virtualdesk_absolute(vx + vw - 1, vy + vh - 1, vx, vy, vw, vh);
            assert_eq!(br_x, 65535, "{label}: bottom-right x ≠ 65535 (got {br_x})");
            assert_eq!(br_y, 65535, "{label}: bottom-right y ≠ 65535 (got {br_y})");
        }
    }

    #[test]
    fn reporter_pixel_lands_on_secondary_monitor() {
        // Issue #1979's exact case: with the secondary monitor occupying
        // (-1920..0, 0..1080), a click at screen (-1795, 383) MUST normalize
        // into the FIRST HALF of the [0, 65535] X band — that's the half of
        // the virtual desktop owned by the secondary monitor. If the math
        // sign-flips, the click lands on the primary monitor instead.
        let (dx, dy) = to_virtualdesk_absolute(-1795, 383, -1920, 0, 3840, 1080);
        assert!(
            dx < 65535 / 2,
            "reporter pixel: dx={dx} fell on PRIMARY half (>= 32767); should be on SECONDARY half"
        );
        // Sanity floor: 125 px in on a 3840 px wide virt → dx ≈ 2133. Allow
        // a generous band so the test isn't brittle to rounding tweaks.
        assert!(
            (2000..2400).contains(&dx),
            "reporter pixel: dx={dx} outside expected ~2133 ± 200 band"
        );
        assert!(
            (23000..24000).contains(&dy),
            "reporter pixel: dy={dy} outside expected ~23247 ± rounding band"
        );
        // Round-trip recovers the input within ±1 (the strongest guarantee).
        let (rx, ry) = from_virtualdesk_absolute(dx, dy, -1920, 0, 3840, 1080);
        assert!(
            (rx - (-1795)).abs() <= 1,
            "round-trip x drift: {rx} vs -1795"
        );
        assert!((ry - 383).abs() <= 1, "round-trip y drift: {ry} vs 383");
    }

    #[test]
    fn pixels_to_the_left_of_seam_stay_on_secondary_half() {
        // The (-1, 0) ↔ (0, 0) seam between secondary (left) and primary
        // (right) is exactly where a sign-flip bug surfaces. Every pixel
        // with sx < 0 in the reporter's layout must produce dx < 32768.
        for sx in [-1920, -1500, -1000, -500, -100, -1] {
            let (dx, _) = to_virtualdesk_absolute(sx, 540, -1920, 0, 3840, 1080);
            assert!(
                dx < 32768,
                "sx={sx} (secondary monitor) produced dx={dx} which is in the PRIMARY half"
            );
        }
        // And every pixel with sx >= 0 must produce dx >= 32768.
        for sx in [0, 100, 500, 1000, 1919] {
            let (dx, _) = to_virtualdesk_absolute(sx, 540, -1920, 0, 3840, 1080);
            assert!(
                dx >= 32768,
                "sx={sx} (primary monitor) produced dx={dx} which is in the SECONDARY half"
            );
        }
    }
}
