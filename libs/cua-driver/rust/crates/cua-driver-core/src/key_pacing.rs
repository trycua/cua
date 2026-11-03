//! Host-chosen gap between synthesized key events (macOS only).
//!
//! The macOS keyboard adapter sleeps a fixed gap between each key down and
//! up and between consecutive keys. That gap is most of the latency of a
//! `type_text` or `hotkey` call. An embedding host whose targets accept
//! faster input can shorten it at trusted launch through the daemon
//! environment:
//!
//! - [`KEY_GAP_ENV`] (`CUA_DRIVER_KEY_GAP_MS`): the gap in milliseconds.
//!
//! Unset, empty, or unparsable values keep the adapter's default, so a daemon
//! launched without the variable behaves exactly as before. The value is read
//! from the daemon process environment only; no tool argument can change it.
//! Linux and Windows keep their own fixed pacing and do not read it. See
//! `Skills/cua-driver/EMBEDDING.md` for what a shorter gap costs.

use std::time::Duration;

/// Gap between synthesized key events, in milliseconds.
pub const KEY_GAP_ENV: &str = "CUA_DRIVER_KEY_GAP_MS";

/// Smallest accepted gap. Smaller values (including `0`) are raised, because
/// some targets coalesce or drop events posted back to back.
pub const MIN_KEY_GAP: Duration = Duration::from_millis(2);

/// Largest accepted gap. Larger values are clamped so a typo cannot make
/// every keystroke wait for seconds.
pub const MAX_KEY_GAP: Duration = Duration::from_millis(100);

/// Resolve the key gap from a raw environment value. Pure, so it can be
/// tested without touching the process environment.
pub fn key_gap_from_raw(raw: Option<&str>, default_gap: Duration) -> Duration {
    raw.and_then(|r| r.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default_gap)
        .clamp(MIN_KEY_GAP, MAX_KEY_GAP)
}

/// Resolve the key gap from the daemon environment, falling back to the
/// adapter's default when unset or unparsable.
pub fn key_gap_from_env(default_gap: Duration) -> Duration {
    key_gap_from_raw(std::env::var(KEY_GAP_ENV).ok().as_deref(), default_gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Duration = Duration::from_millis(8);

    #[test]
    fn unset_keeps_adapter_default() {
        assert_eq!(key_gap_from_raw(None, DEFAULT), DEFAULT);
    }

    #[test]
    fn valid_values_are_honored() {
        assert_eq!(
            key_gap_from_raw(Some("4"), DEFAULT),
            Duration::from_millis(4)
        );
        assert_eq!(
            key_gap_from_raw(Some(" 20\n"), DEFAULT),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn invalid_values_fall_back_to_default() {
        for raw in ["", "  ", "abc", "-1", "1.5", "8ms", "18446744073709551616"] {
            assert_eq!(key_gap_from_raw(Some(raw), DEFAULT), DEFAULT, "{raw:?}");
        }
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        assert_eq!(key_gap_from_raw(Some("0"), DEFAULT), MIN_KEY_GAP);
        assert_eq!(key_gap_from_raw(Some("1"), DEFAULT), MIN_KEY_GAP);
        assert_eq!(key_gap_from_raw(Some("60000"), DEFAULT), MAX_KEY_GAP);
        assert_eq!(
            key_gap_from_raw(Some(&u64::MAX.to_string()), DEFAULT),
            MAX_KEY_GAP
        );
    }
}
