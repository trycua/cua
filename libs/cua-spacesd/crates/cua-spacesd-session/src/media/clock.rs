// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The driver-wide media clock (MEDIA.md §12.1).
//!
//! Every video `capture_timestamp_us` and audio `pts_us` is in microseconds on
//! one monotonic clock per driver. The clock is `cua_media_codec`'s
//! `media_clock_us`, so audio packets stamped by the codec layer and video
//! frames stamped here share one time base. Values are u64 and never wrap.

use std::time::{Duration, Instant};

/// Microseconds since the media clock origin.
pub fn now_us() -> u64 {
    cua_media_codec::media_clock_us()
}

/// Map a monotonic instant onto the media clock. Instants before the origin
/// map to 0.
pub fn to_media_us(instant: Instant) -> u64 {
    let now = Instant::now();
    let now_us = now_us();
    if instant <= now {
        now_us.saturating_sub(
            u64::try_from(now.duration_since(instant).as_micros()).unwrap_or(u64::MAX),
        )
    } else {
        now_us.saturating_add(
            u64::try_from(instant.duration_since(now).as_micros()).unwrap_or(u64::MAX),
        )
    }
}

/// The monotonic instant of a media-clock value.
pub fn from_media_us(us: u64) -> Instant {
    let now = Instant::now();
    let now_us = now_us();
    if us <= now_us {
        now - Duration::from_micros(now_us - us)
    } else {
        now + Duration::from_micros(us - now_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_monotonic_shared_with_the_codec_layer_and_round_trips() {
        let a = now_us();
        let b = cua_media_codec::media_clock_us();
        assert!(b >= a);
        let instant = Instant::now();
        let us = to_media_us(instant);
        let back = from_media_us(us);
        let skew = if back > instant {
            back - instant
        } else {
            instant - back
        };
        assert!(skew < Duration::from_millis(2), "{skew:?}");
    }
}
