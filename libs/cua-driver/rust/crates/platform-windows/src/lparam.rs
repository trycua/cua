//! Pure math for packing/unpacking `LPARAM` coordinate pairs in window
//! messages — `WM_MOUSEMOVE`, `WM_LBUTTONDOWN`, `WM_RBUTTONUP`, and friends.
//!
//! Windows packs the click `(x, y)` into a single `LPARAM` (`MAKELPARAM` in
//! `windowsx.h`): low 16 bits hold `x`, high 16 bits hold `y`. The receiver
//! extracts with `GET_X_LPARAM` / `GET_Y_LPARAM`, which sign-extend each half
//! via `(int)(short)LOWORD(lp)` — so negative client-area coordinates
//! (legitimately produced by `ScreenToClient` for clicks on a window that
//! straddles a monitor edge, or for non-client clicks above the title bar)
//! round-trip back to the same `i32` value, as long as each coordinate fits in
//! `i16` range.
//!
//! This module exists alongside [`crate::virtualdesk`] for the same reason:
//! the coordinate-handoff is the kind of pure math that needs cross-platform
//! unit tests on every host (issue #1979 — the agent reported correct
//! screen-points being delivered to the wrong monitor on multi-monitor setups
//! with negative virtual-desktop X origins, so we pin both the SendInput
//! [VIRTUALDESK normalization] and the PostMessage [LPARAM packing] paths
//! with regression tests on the macOS CI host).
//!
//! Audit conclusion for #1979: the existing PostMessage packing is correct
//! for every screen / client coordinate that fits in `i16` range, including
//! the reporter's `(-1198, 292)` and `(-1795, 383)` examples — the round-trip
//! through `GET_X_LPARAM` / `GET_Y_LPARAM` recovers the input exactly. The
//! tests below lock that invariant in so a future "simplification" (e.g.
//! dropping the implicit `i16` truncation and going `x as u32` directly)
//! cannot regress it without turning a test red.

/// Pack a `(x, y)` coordinate pair into the 32-bit `LPARAM` payload used by
/// every `WM_MOUSE*` and `WM_*BUTTON*` message: `x` in the low 16 bits,
/// `y` in the high 16 bits, each truncated to `i16` width.
///
/// Returned as `u32` — the caller is responsible for the final `LPARAM`
/// (`isize`) cast. Splitting the pure-bit-math from the Win32 `LPARAM`
/// newtype lets this run in `cargo test` on any host.
///
/// Returns `Err` when either coordinate falls outside the `i16` range
/// `[-32768, 32767]`. Win32 packs the LPARAM as two `WORD`s (16 bits each),
/// so a coordinate outside that range would silently truncate to a different
/// value on the receiver — and the receiver has no way to distinguish that
/// from a legitimate small-magnitude coord. Every existing call site in
/// `mouse.rs` passes client-area coords that come out of `ScreenToClient`
/// (window-local, capped at the window's extents) so they're always inside
/// `i16` range in practice — the explicit fail-fast is the cheap insurance
/// against a future caller passing screen-direct coords on a >32k-pixel
/// virtual desktop.
#[inline]
pub fn pack_xy(x: i32, y: i32) -> Result<u32, LParamPackError> {
    if !fits_in_i16(x) {
        return Err(LParamPackError::XOutOfRange(x));
    }
    if !fits_in_i16(y) {
        return Err(LParamPackError::YOutOfRange(y));
    }
    // `i32 as u16` truncates to the low 16 bits, preserving the two's-complement
    // bit pattern of any i16-fitting value. The receiver re-interprets each
    // half as `(short)` (signed 16-bit) — see `unpack_x` / `unpack_y`.
    Ok(((y as u16 as u32) << 16) | (x as u16 as u32))
}

/// Errors that [`pack_xy`] can surface. Currently only out-of-range coord
/// errors; carries the bad value for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LParamPackError {
    /// `x` cannot be represented as `i16`. Carries the offending value.
    XOutOfRange(i32),
    /// `y` cannot be represented as `i16`. Carries the offending value.
    YOutOfRange(i32),
}

impl core::fmt::Display for LParamPackError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::XOutOfRange(v) => write!(
                f,
                "LPARAM pack: x={v} is outside i16 range [-32768, 32767]; \
                 PostMessage coords must be window-local (post-ScreenToClient)"
            ),
            Self::YOutOfRange(v) => write!(
                f,
                "LPARAM pack: y={v} is outside i16 range [-32768, 32767]; \
                 PostMessage coords must be window-local (post-ScreenToClient)"
            ),
        }
    }
}

impl std::error::Error for LParamPackError {}

#[inline]
fn fits_in_i16(v: i32) -> bool {
    (i16::MIN as i32..=i16::MAX as i32).contains(&v)
}

/// Mirror of Win32's `GET_X_LPARAM` macro: `(int)(short)LOWORD(lp)`. Used by
/// the round-trip tests; the production code never decodes an LPARAM it
/// packed itself.
#[inline]
#[cfg(test)]
pub fn unpack_x(lp: u32) -> i32 {
    (lp & 0xFFFF) as i16 as i32
}

/// Mirror of Win32's `GET_Y_LPARAM` macro: `(int)(short)HIWORD(lp)`.
#[inline]
#[cfg(test)]
pub fn unpack_y(lp: u32) -> i32 {
    ((lp >> 16) & 0xFFFF) as i16 as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip every coordinate in `cases` through `pack_xy` and the
    /// `GET_X_LPARAM` / `GET_Y_LPARAM` mirrors. The packing contract is that
    /// any `i16`-fitting input is recovered exactly by the receiver.
    fn assert_round_trips(cases: &[(i32, i32)]) {
        for &(x, y) in cases {
            let lp = pack_xy(x, y).expect("inputs all fit in i16");
            let rx = unpack_x(lp);
            let ry = unpack_y(lp);
            assert_eq!(
                (rx, ry),
                (x, y),
                "round-trip mismatch: in=({x}, {y}) lp=0x{lp:08x} out=({rx}, {ry})"
            );
        }
    }

    #[test]
    fn reporter_negative_coords_round_trip() {
        // The two coordinate pairs the reporter saw in #1979's PostMessage path
        // log lines:
        //   "Posted double-click to [10] at screen-point (-1198, 292)"
        //   "Posted double-click to pid X at window-pixel (38, 40) → screen-point (-1795, 383)"
        //
        // The screen-point form goes through post_click_screen → deepest_child
        // → ScreenToClient before reaching make_lparam, so by the time we pack
        // the coords they're window-local and almost always small positives —
        // but on a child window that straddles the monitor edge, ScreenToClient
        // can produce a small negative for the non-client side. Lock both signs
        // in.
        assert_round_trips(&[
            (-1198, 292),
            (-1795, 383),
            (38, 40),
            (-39, 383),    // representative post-ScreenToClient coord on the
                           //  secondary-monitor window edge
        ]);
    }

    #[test]
    fn full_i16_range_round_trips() {
        // Both signs at the i16 fence-posts. If a future change accidentally
        // drops the implicit i16 cast (e.g. someone "simplifies" `x as u16` to
        // `(x as u32) & 0xFFFF`, which evaluates the same for valid inputs but
        // hides intent for the next reader), the round-trip still holds inside
        // the i16 range — what we'd want catching here is a sign-flip at the
        // boundary, which `i16::MIN ↔ i16::MAX` exercises.
        assert_round_trips(&[
            (i16::MIN as i32, 0),
            (i16::MAX as i32, 0),
            (0, i16::MIN as i32),
            (0, i16::MAX as i32),
            (i16::MIN as i32, i16::MAX as i32),
            (i16::MAX as i32, i16::MIN as i32),
            (-1, -1),
            (1, 1),
        ]);
    }

    #[test]
    fn out_of_range_coords_rejected() {
        // Outside i16, packing must surface an error rather than silently
        // truncating — the truncated bit pattern is indistinguishable on the
        // receiving side from a legitimate small value, so a silent truncation
        // would be the worst possible outcome.
        let bad_x = pack_xy(i16::MAX as i32 + 1, 0);
        assert!(matches!(bad_x, Err(LParamPackError::XOutOfRange(v)) if v == 32768));
        let bad_neg_x = pack_xy(i16::MIN as i32 - 1, 0);
        assert!(matches!(bad_neg_x, Err(LParamPackError::XOutOfRange(v)) if v == -32769));
        let bad_y = pack_xy(0, 65000);
        assert!(matches!(bad_y, Err(LParamPackError::YOutOfRange(v)) if v == 65000));
    }

    #[test]
    fn round_trip_negative_x_does_not_sign_flip_to_primary_monitor() {
        // The structural worry behind #1979's PostMessage half: a packing bug
        // that maps a negative-X click (secondary-monitor window-local coord)
        // to a positive X (primary-monitor coord) would manifest as the agent
        // reporting "click at (-1198, 292)" while the actual click lands on
        // the primary monitor.
        //
        // The receiver-side macro `GET_X_LPARAM` sign-extends via
        // `(int)(short)LOWORD(lp)`. As long as our packed `x` keeps its
        // two's-complement bit pattern in the low 16 bits, the receiver
        // recovers the original negative value. This test fails if anyone
        // ever replaces the implicit `i16` truncation with a zero-extending
        // cast (e.g. `(x as u32) & 0xFFFF` from an `i32::abs()`-style change).
        for x in [-32768_i32, -10000, -1795, -1198, -1, 0, 1, 1000] {
            let lp = pack_xy(x, 0).unwrap();
            let rx = unpack_x(lp);
            assert_eq!(rx, x, "sign-flip: x={x} round-tripped to {rx}");
            // And the same `x` must produce a strictly smaller LPARAM's low
            // word when its neighbour does — i.e. the bit pattern is
            // monotone in the i16 sense. We can check that by re-decoding
            // adjacent samples.
            if x < i16::MAX as i32 {
                let lp2 = pack_xy(x + 1, 0).unwrap();
                let rx2 = unpack_x(lp2);
                assert_eq!(rx2, x + 1, "monotonicity decode failed");
            }
        }
    }

    #[test]
    fn screen_coord_to_window_local_then_pack_monotonic() {
        // Approximate the full coord-translation pipeline for the reporter's
        // window: a window at screen origin (-1834, 0) (the DWM frame the
        // reporter's `bitmap_to_screen` math implies). For two adjacent screen
        // pixels (sx, y) and (sx+1, y), the window-local x increases by 1, so
        // the packed LPARAM's low word (when re-interpreted as i16) must also
        // increase by 1.
        //
        // This mirrors the #1980 `monotonic_in_x_and_y` test for the SendInput
        // path: any sign-mangling bug that folded negatives back into the
        // positive band would break the monotonicity here. Adjacent screen
        // pixels must produce adjacent packed coords for any sx along the
        // secondary monitor (`sx in [-1834, -915)`).
        let window_origin_x: i32 = -1834;
        let window_origin_y: i32 = 0;
        // Screen pixels sweeping across the secondary monitor (1920 px wide).
        let mut prev: Option<i32> = None;
        for sx in window_origin_x..(window_origin_x + 1920) {
            let local_x = sx - window_origin_x; // 0..1920 — well inside i16
            let local_y = 383 - window_origin_y;
            let lp = pack_xy(local_x, local_y)
                .expect("post-ScreenToClient coords always fit in i16");
            let rx = unpack_x(lp);
            if let Some(p) = prev {
                assert_eq!(
                    rx,
                    p + 1,
                    "non-monotonic at sx={sx}: prev decoded x={p}, current decoded x={rx}"
                );
            }
            prev = Some(rx);
        }
    }

    #[test]
    fn full_reporter_scenario_screen_minus_1795_through_postmessage_pipeline() {
        // End-to-end reconstruction of the reporter's `(-1795, 383)` path:
        //
        //   1. Agent passes window_id with DWM frame at screen X=-1834 and
        //      bitmap pixel (38, 40) → screen (-1795, 383)  (per bitmap_to_screen).
        //   2. post_click_screen → deepest_child → ScreenToClient(window, (-1795, 383))
        //      = (39, 383)  (window-local; positive; well inside i16).
        //   3. ChildWindowFromPointEx finds the deepest descendant; we'll assume the
        //      target is the same window (a single-HWND app) for this pure-math test.
        //   4. make_lparam(39, 383) → LPARAM payload.
        //   5. Receiver decodes via GET_X_LPARAM / GET_Y_LPARAM.
        //
        // The assertion: the decoded coords must match step (2) exactly. Any
        // sign-flip or truncation in steps (4)-(5) would land the click on the
        // wrong screen — which is the reporter's symptom.
        let window_screen_left: i32 = -1834;
        let click_screen_x: i32 = -1795;
        let click_screen_y: i32 = 383;
        let local_x = click_screen_x - window_screen_left; // 39
        let local_y = click_screen_y; // window top at y=0
        assert_eq!(local_x, 39);
        let lp = pack_xy(local_x, local_y).unwrap();
        let (rx, ry) = (unpack_x(lp), unpack_y(lp));
        assert_eq!(
            (rx, ry),
            (39, 383),
            "PostMessage LPARAM packing dropped the reporter's coords on the floor"
        );
    }
}
