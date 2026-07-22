//! Privacy-safe human input observations for cua-driver demonstrations.
//!
//! Windows capture is implemented. Other platforms return
//! [`CaptureError::Unsupported`]. The public [`Demonstration`] guard owns the
//! indicator and input hooks as one lifecycle so capture cannot outlive its
//! user-visible notification.

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod coalesce;
mod demonstration;
mod event;
mod indicator;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod render_health;

pub use demonstration::Demonstration;
pub use event::{Button, HumanEvent, Modifiers};

use std::sync::mpsc::SyncSender;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub pid: i64,
    pub window_id: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("human input capture is not supported on this platform yet")]
    Unsupported,
    #[error("failed to install input hook: {0}")]
    Hook(String),
}

pub(crate) fn start(
    config: CaptureConfig,
    health: render_health::RenderHealth,
    sink: SyncSender<HumanEvent>,
    started_at: Instant,
) -> Result<platform::Capture, CaptureError> {
    platform::Capture::start(config, health, sink, started_at)
}

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;

    pub(crate) struct Capture;

    impl Capture {
        pub(crate) fn start(
            _config: CaptureConfig,
            _health: render_health::RenderHealth,
            _sink: SyncSender<HumanEvent>,
            _started_at: Instant,
        ) -> Result<Self, CaptureError> {
            Err(CaptureError::Unsupported)
        }

        pub(crate) fn stop(self) {}
    }
}
