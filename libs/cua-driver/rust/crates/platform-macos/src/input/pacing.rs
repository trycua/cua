//! macOS defaults for the launch-time input pacing knobs.
//!
//! Each gap keeps its previous hardcoded value as the default. A host can
//! override it through the daemon environment; parsing and bounds live in
//! `cua_driver_core::input_pacing`. Read once per process: pacing is a
//! launch-time knob, not a per-call one.

use cua_driver_core::input_pacing::{
    PacingKnob, CLICK_GAP, MOUSE_PRIMER, MULTI_CLICK_GAP, TYPE_TEXT_DELAY, WEBKIT_SETTLE,
};
use std::sync::OnceLock;
use std::time::Duration;

fn resolve(cell: &'static OnceLock<Duration>, knob: PacingKnob, default_ms: u64) -> Duration {
    *cell.get_or_init(|| knob.from_env(Duration::from_millis(default_ms)))
}

/// Settle after the leading mouseMoved primer. Default 12 ms.
pub(crate) fn mouse_primer_settle() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    resolve(&CELL, MOUSE_PRIMER, 12)
}

/// Mouse down→up gap within one click. Default 28 ms.
pub(crate) fn click_gap() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    resolve(&CELL, CLICK_GAP, 28)
}

/// Gap between the down/up pairs of a multi-click. Default 80 ms.
pub(crate) fn multi_click_gap() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    resolve(&CELL, MULTI_CLICK_GAP, 80)
}

/// WebKit DOM focus settle after an AX press on a text input. Default 800 ms.
pub(crate) fn webkit_settle() -> Duration {
    static CELL: OnceLock<Duration> = OnceLock::new();
    resolve(&CELL, WEBKIT_SETTLE, 800)
}

/// `type_text`'s default `delay_ms` when the caller omits it. Default 30 ms.
pub(crate) fn type_text_default_delay_ms() -> u64 {
    static CELL: OnceLock<Duration> = OnceLock::new();
    resolve(&CELL, TYPE_TEXT_DELAY, 30).as_millis() as u64
}
