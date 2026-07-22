//! Lifecycle coupling for the recording indicator and platform input capture.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Instant;

use crate::indicator::Indicator;
use crate::render_health::RenderHealth;
use crate::{
    start as start_capture, validate, CaptureConfig, CaptureError, CaptureStats, HumanEvent,
};

/// A live human demonstration. Drop it to stop capture and remove the border.
pub struct Demonstration {
    // Drop capture before the indicator so input never outlives notification.
    capture: Option<crate::platform::Capture>,
    _indicator: Indicator,
    dropped_events: Arc<AtomicUsize>,
}

impl Demonstration {
    pub fn start(
        config: CaptureConfig,
        sink: SyncSender<HumanEvent>,
    ) -> Result<Self, CaptureError> {
        validate(&config)?;
        let started_at = Instant::now();
        let health = RenderHealth::new();
        let indicator = Indicator::start(
            config.window_id as isize,
            config.pid as u32,
            health.clone(),
            started_at,
        )
        .map_err(|error| CaptureError::Hook(format!("indicator: {error}")))?;

        // Install hooks only after the border submits its first frame.
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        while !health.is_fresh(started_at.elapsed().as_millis() as u64) {
            if Instant::now() >= deadline {
                return Err(CaptureError::Hook(
                    "indicator did not render within one second".into(),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let dropped_events = Arc::new(AtomicUsize::new(0));
        let capture = start_capture(config, health, sink, started_at, dropped_events.clone())?;
        Ok(Self {
            capture: Some(capture),
            _indicator: indicator,
            dropped_events,
        })
    }

    pub fn stop(mut self) -> CaptureStats {
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        CaptureStats {
            dropped_events: self.dropped_events.load(Ordering::Acquire),
        }
    }
}

impl Drop for Demonstration {
    fn drop(&mut self) {
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
    }
}
