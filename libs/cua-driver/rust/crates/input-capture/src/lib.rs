//! Window-scoped human input capture for cua-driver demonstration recording.
//!
//! This crate captures what a **human** does to a single target window during a
//! demonstration — clicks, scrolls, drags, and (coalesced, redacted) typing —
//! so a recording can include both agent tool-calls and human actions on one
//! timeline. It is deliberately NOT a system-wide keylogger:
//!
//! * Capture is **window-scoped**: events outside the target window are dropped
//!   inside the hook callback before anything is buffered (see the platform
//!   backends' foreground checks).
//! * Capture is **indicator-gated**: every event must pass a [`CaptureGate`]
//!   proving the visible recording border is live and covering the point. See
//!   [`gate`] for the coupling invariant.
//! * Typed text is **redacted by default**; raw capture is explicit opt-in.
//!
//! Platform support: Windows implemented; macOS/Linux return
//! [`CaptureError::Unsupported`] for now.

mod coalesce;
mod demonstration;
mod event;
mod gate;
mod indicator;

pub use coalesce::{Coalescer, KeyInput, DEFAULT_IDLE_FLUSH_MS};
pub use demonstration::Demonstration;
pub use event::{redact, Button, HumanEvent, Mods};
pub use gate::{CaptureGate, IndicatorHeartbeat, DEFAULT_MAX_FRAME_AGE_MS};
pub use indicator::Indicator;

use std::sync::mpsc::Sender;

/// Identifies the single window a demonstration is scoped to.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub pid: i64,
    pub window_id: u64,
    /// Capture literal typed text in addition to the redacted summary. Off by
    /// default; stores secrets when on, so callers must opt in and disclose it.
    pub capture_raw_text: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("human input capture is not supported on this platform yet")]
    Unsupported,
    #[error("failed to install input hook: {0}")]
    Hook(String),
}

/// A running, window-scoped, indicator-gated human input capture. Dropping it
/// uninstalls the platform hook and flushes any pending text run.
pub trait InputCapture: Send {
    /// Stop capturing and release the hook. Idempotent.
    fn stop(self: Box<Self>);
}

/// Start capturing human input on `cfg`'s window, gated on `gate` (which must
/// be derived from the live recording indicator's heartbeat). Captured events
/// are sent on `sink`. The `clock` returns monotonic-ms matching the recording
/// session anchor so human events interleave correctly with agent actions.
///
/// There is intentionally no overload that omits `gate`: a hook cannot be
/// installed without a capture gate, and a gate cannot exist without an
/// indicator heartbeat. See [`gate`].
pub fn start(
    cfg: CaptureConfig,
    gate: CaptureGate,
    sink: Sender<HumanEvent>,
    clock: impl Fn() -> u64 + Send + 'static,
) -> Result<Box<dyn InputCapture>, CaptureError> {
    platform::start(cfg, gate, sink, clock)
}

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;
    pub fn start(
        _cfg: CaptureConfig,
        _gate: CaptureGate,
        _sink: Sender<HumanEvent>,
        _clock: impl Fn() -> u64 + Send + 'static,
    ) -> Result<Box<dyn InputCapture>, CaptureError> {
        // macOS: planned via CGEventTap + Accessibility grant.
        // Linux:  planned via XInput2 / libei.
        Err(CaptureError::Unsupported)
    }
}
