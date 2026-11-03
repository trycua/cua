// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The largest frame an H.264 stream encodes: the VideoToolbox hardware
//! encoder (and H.264 level 5.2) tops out at 4096 px on the long edge and
//! 36,864 macroblocks (4096x2304). A larger source, such as a 6K display
//! (6720x3780), is captured downscaled to fit instead of failing to encode.

/// Longest edge, in pixels, of an H.264 frame.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const H264_MAX_DIMENSION: u32 = 4096;
/// Most 16x16 macroblocks in one H.264 frame (level 5.2 MaxFS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const H264_MAX_MACROBLOCKS: u64 = 36_864;

/// The long-edge cap an H.264 stream of a `width`x`height` (native px)
/// source captures at: `requested` (0 = native size) lowered, when needed,
/// so the downscaled frame fits [`H264_MAX_DIMENSION`] and
/// [`H264_MAX_MACROBLOCKS`]. Never 0.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn h264_max_dimension(width: u32, height: u32, requested: u32) -> u32 {
    let (width, height) = (width.max(1), height.max(1));
    let longest = width.max(height);
    let mut cap = match requested {
        0 => longest,
        r => r.min(longest),
    }
    .min(H264_MAX_DIMENSION);
    // The frame area shrinks with the square of the cap: start from the
    // cap whose area fits, then step down (bounded) past rounding.
    if macroblocks(width, height, cap) > H264_MAX_MACROBLOCKS {
        let fit =
            (H264_MAX_MACROBLOCKS as f64 * 256.0 / (f64::from(width) * f64::from(height))).sqrt();
        // (The epsilon keeps an exact fit, 3072 for 5000x5000, from
        // rounding down a pixel.)
        cap = cap
            .min((f64::from(longest) * fit + 1e-6).floor() as u32)
            .max(16);
        for _ in 0..64 {
            if macroblocks(width, height, cap) <= H264_MAX_MACROBLOCKS || cap <= 16 {
                break;
            }
            cap -= 1;
        }
    }
    cap.max(1)
}

/// Macroblocks of `width`x`height` scaled so its long edge is at most `cap`.
fn macroblocks(width: u32, height: u32, cap: u32) -> u64 {
    let longest = width.max(height);
    let (w, h) = if longest <= cap {
        (width, height)
    } else {
        let ratio = f64::from(cap) / f64::from(longest);
        (
            ((f64::from(width) * ratio).round() as u32).max(1),
            ((f64::from(height) * ratio).round() as u32).max(1),
        )
    };
    u64::from(w.div_ceil(16)) * u64::from(h.div_ceil(16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fits(width: u32, height: u32, requested: u32) {
        let cap = h264_max_dimension(width, height, requested);
        assert!(cap <= H264_MAX_DIMENSION, "{width}x{height}: {cap}");
        assert!(
            macroblocks(width, height, cap) <= H264_MAX_MACROBLOCKS,
            "{width}x{height}: {cap}"
        );
    }

    /// A 6K main display (6720x3780) streamed at native size (the viewer's
    /// `max_dimension` 0) is captured at 4096x2304, which H.264 encodes
    /// (#4623); before, the 6720x3780 frame went to the encoder as is.
    #[test]
    fn a_6k_display_is_downscaled_to_the_h264_limit() {
        assert_eq!(h264_max_dimension(6720, 3780, 0), 4096);
        assert_eq!(macroblocks(6720, 3780, 4096), 36_864);
        // Portrait 5K (2880x5120): 2304x4096.
        assert_eq!(h264_max_dimension(2880, 5120, 0), 4096);
        // A square source over the macroblock budget shrinks further.
        assert_eq!(h264_max_dimension(5000, 5000, 0), 3072);
        for (w, h) in [
            (6720, 3780),
            (7680, 4320),
            (5120, 2880),
            (5000, 5000),
            (3840, 3840),
        ] {
            for requested in [0, 1280, 4096, 8192] {
                fits(w, h, requested);
            }
        }
    }

    #[test]
    fn a_frame_within_the_limit_keeps_the_requested_cap() {
        assert_eq!(h264_max_dimension(2560, 1440, 0), 2560);
        assert_eq!(h264_max_dimension(2560, 1440, 1920), 1920);
        assert_eq!(h264_max_dimension(6720, 3780, 1600), 1600);
        assert_eq!(h264_max_dimension(3840, 2160, 0), 3840);
    }
}
