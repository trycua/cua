//! `Demonstration` — the RAII guard that structurally couples the visible
//! recording indicator to the input hook.
//!
//! This is the single supported entry point for human-input demonstration
//! capture. It constructs the [`Indicator`] first, derives a [`CaptureGate`]
//! from the indicator's heartbeat, and only then installs the input hook with
//! that gate. There is no way to get a running hook without first having a
//! running indicator. Dropping the guard stops the hook, then the indicator —
//! and even between drops, the gate keeps capture dark whenever the indicator
//! is not provably painting (see [`crate::gate`]).

use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::gate::{CaptureGate, IndicatorHeartbeat};
use crate::indicator::Indicator;
use crate::{start as start_capture, CaptureConfig, CaptureError, HumanEvent, InputCapture};

/// A live demonstration: indicator border + gated input hook. Drop to stop.
pub struct Demonstration {
    // Field order matters: `capture` (the hook) is declared before `indicator`
    // so it is dropped first — we stop recording input before removing the
    // border, never the other way around.
    capture: Option<Box<dyn InputCapture>>,
    _indicator: Indicator,
}

impl Demonstration {
    /// Begin a demonstration on `cfg`'s window. Captured human events are sent
    /// on `sink`; `clock` is the shared monotonic-ms clock (same anchor as the
    /// recording session's agent actions).
    pub fn start(
        cfg: CaptureConfig,
        sink: Sender<HumanEvent>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Self, CaptureError> {
        let heartbeat = IndicatorHeartbeat::new();

        // 1. Indicator FIRST — the gate is derived from its heartbeat.
        let indicator = Indicator::start(cfg.window_id as isize, heartbeat.clone(), clock.clone())
            .map_err(|e| CaptureError::Hook(format!("indicator: {e}")))?;

        // 2. Gate from the live heartbeat; the hook can only start with this.
        let gate = CaptureGate::new(heartbeat);

        // 3. Hook last. If it fails, `indicator` drops and tears the border down.
        let clock_for_hook = clock.clone();
        let capture = start_capture(cfg, gate, sink, move || clock_for_hook())?;

        Ok(Self { capture: Some(capture), _indicator: indicator })
    }

    /// Explicitly stop (idempotent). Equivalent to dropping the guard but lets
    /// the caller join the hook teardown synchronously before continuing.
    pub fn stop(mut self) {
        if let Some(c) = self.capture.take() {
            c.stop();
        }
        // `_indicator` drops here.
    }
}

impl Drop for Demonstration {
    fn drop(&mut self) {
        if let Some(c) = self.capture.take() {
            c.stop();
        }
    }
}
