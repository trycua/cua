//! Deploy-time input pacing knobs.
//!
//! Every gap here defaults to the previously hardcoded constant — zero
//! out-of-the-box behavior change — and is overridable via a
//! `CUA_DRIVER_RS_*` env var for latency-sensitive deployments, following
//! the crate's existing override pattern (`CUA_DRIVER_RS_DRAW_SYSTEM_CURSOR`
//! et al.). Read once per process: pacing is a deploy-time knob, not a
//! per-call one.

use std::sync::OnceLock;
use std::time::Duration;

/// The duration for `env_value`, falling back to `default_ms` when unset
/// or unparseable. Pure so the fallback contract is unit-testable without
/// process-global env mutation.
fn ms_from(env_value: Option<&str>, default_ms: u64) -> Duration {
    env_value
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(default_ms))
}

fn env_ms(cell: &'static OnceLock<Duration>, name: &str, default_ms: u64) -> Duration {
    *cell.get_or_init(|| ms_from(std::env::var(name).ok().as_deref(), default_ms))
}

/// Settle after the leading mouseMoved primer.
/// `CUA_DRIVER_RS_MOUSE_PRIMER_MS`, default 12 (the previous constant).
pub(crate) fn mouse_primer_settle() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    env_ms(&CELL, "CUA_DRIVER_RS_MOUSE_PRIMER_MS", 12)
}

/// Mouse down→up gap within one click.
/// `CUA_DRIVER_RS_CLICK_GAP_MS`, default 28 (the previous constant).
pub(crate) fn click_gap() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    env_ms(&CELL, "CUA_DRIVER_RS_CLICK_GAP_MS", 28)
}

/// Gap between the down/up pairs of a multi-click.
/// `CUA_DRIVER_RS_MULTI_CLICK_GAP_MS`, default 80 (the previous constant).
pub(crate) fn multi_click_gap() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    env_ms(&CELL, "CUA_DRIVER_RS_MULTI_CLICK_GAP_MS", 80)
}

/// WebKit DOM focus settle after clicking a text input.
/// `CUA_DRIVER_RS_WEBKIT_SETTLE_MS`, default 800 (the previous constant).
pub(crate) fn webkit_settle() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    env_ms(&CELL, "CUA_DRIVER_RS_WEBKIT_SETTLE_MS", 800)
}

/// `type_text`'s default inter-character delay in whole ms (feeds the
/// tool's `delay_ms` argument when the caller omits it).
/// `CUA_DRIVER_RS_TYPE_TEXT_DELAY_MS`, default 30 (the previous constant).
pub(crate) fn type_text_default_delay_ms() -> u64 {
    static CELL: OnceLock<Duration> = OnceLock::new();
    env_ms(&CELL, "CUA_DRIVER_RS_TYPE_TEXT_DELAY_MS", 30).as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every knob honors its env value and falls back to the default on
    /// unset or unparseable input.
    #[test]
    fn ms_from_parses_and_falls_back() {
        assert_eq!(ms_from(Some("4"), 28), Duration::from_millis(4));
        assert_eq!(ms_from(Some("0"), 28), Duration::from_millis(0));
        assert_eq!(ms_from(Some("junk"), 28), Duration::from_millis(28));
        assert_eq!(ms_from(Some(""), 28), Duration::from_millis(28));
        assert_eq!(ms_from(None, 28), Duration::from_millis(28));
    }
}
