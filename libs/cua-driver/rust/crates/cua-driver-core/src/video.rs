//! Cross-platform video-capture abstraction.
//!
//! The recording session calls into a single `VideoBackend` trait; the
//! concrete implementation is selected at process startup by the
//! platform crate. Today:
//!
//! - **macOS:** native ScreenCaptureKit via `platform_macos::video_sckit`
//!   (no extra TCC grant — inherits cua-driver's own Screen Recording
//!   permission, no subprocess).
//! - **Windows + Linux:** ffmpeg subprocess via `video_ffmpeg`
//!   (`gdigrab` / `x11grab` input + libx264 encode).
//!
//! The factory is registered with `set_video_backend_factory` from each
//! platform's `main.rs` startup block, mirroring how `SCREENSHOT_FN` /
//! `AX_SNAPSHOT_FN` are wired in `recording.rs`.
//!
//! `VideoMetadata` is the shape `RecordingSession` stamps into
//! `session.json` after `stop()` — kept identical to the prior concrete
//! `VideoRecorder::stop` return so the on-disk schema is unchanged.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "WindowTargetWire", into = "WindowTargetWire")]
pub struct WindowVideoTarget {
    pub pid: i32,
    pub window_id: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WindowTargetWire {
    Window { pid: i32, window_id: u64 },
}

impl WindowVideoTarget {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.pid > 0 && self.window_id > 0,
            "invalid_recording_target: pid and window_id must be positive"
        );
        Ok(())
    }
}

impl TryFrom<WindowTargetWire> for WindowVideoTarget {
    type Error = anyhow::Error;
    fn try_from(wire: WindowTargetWire) -> anyhow::Result<Self> {
        let WindowTargetWire::Window { pid, window_id } = wire;
        let target = Self { pid, window_id };
        target.validate()?;
        Ok(target)
    }
}

impl From<WindowVideoTarget> for WindowTargetWire {
    fn from(target: WindowVideoTarget) -> Self {
        Self::Window {
            pid: target.pid,
            window_id: target.window_id,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowVideoInfo {
    pub target: WindowVideoTarget,
    pub width: u32,
    pub height: u32,
    pub backend: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowVideoStatus {
    pub info: WindowVideoInfo,
    pub active: bool,
    pub finalized: bool,
    pub duration_ms: u64,
    pub termination_reason: Option<String>,
    pub error: Option<String>,
}

pub type WindowVideoObserver = Arc<dyn Fn(WindowVideoStatus) + Send + Sync>;

pub trait PreparedWindowVideo: Send {
    fn info(&self) -> WindowVideoInfo;
    fn start(
        self: Box<Self>,
        path: &Path,
        observer: WindowVideoObserver,
    ) -> anyhow::Result<Box<dyn VideoBackend>>;
}

/// Finalized metadata returned by `VideoBackend::stop`. Mirrors the
/// Swift impl's `FinalMetadata` so `session.json` carries the same
/// shape across all backends.
#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub path: PathBuf,
    /// Wall-clock duration the recorder was active.
    pub duration_ms: u64,
    /// Whether the backend finalized the mp4 cleanly (playable file).
    pub finalized: bool,
}

/// One active capture session. Owned by `RecordingSession` for the
/// session's lifetime; `stop()` consumes it and finalizes the file.
pub trait VideoBackend: Send {
    fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata>;
    fn window_status(&self) -> Option<WindowVideoStatus> {
        None
    }
}

/// Spawns a fresh `VideoBackend` writing to `output_path`. Registered
/// once at startup via `set_video_backend_factory`.
pub trait VideoBackendFactory: Send + Sync {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>>;
    fn prepare_window(
        &self,
        _target: &WindowVideoTarget,
    ) -> anyhow::Result<Box<dyn PreparedWindowVideo>> {
        anyhow::bail!("window_recording_unsupported: this backend does not support window video")
    }
}

static VIDEO_BACKEND_FACTORY: OnceLock<Box<dyn VideoBackendFactory>> = OnceLock::new();

pub fn prepare_window_video(
    target: &WindowVideoTarget,
) -> anyhow::Result<Box<dyn PreparedWindowVideo>> {
    target.validate()?;
    VIDEO_BACKEND_FACTORY
        .get()
        .ok_or_else(|| {
            anyhow::anyhow!("window_recording_unsupported: no video backend registered")
        })?
        .prepare_window(target)
}

/// Register the platform's video backend. Idempotent — subsequent calls
/// are silently ignored, matching the other recording-callback setters.
pub fn set_video_backend_factory(factory: Box<dyn VideoBackendFactory>) {
    let _ = VIDEO_BACKEND_FACTORY.set(factory);
}

/// Start a video capture using the registered backend. Returns an error
/// when no backend has been registered for this platform (treated by
/// `RecordingSession` as "video failed to start" — the per-turn pipeline
/// keeps running).
pub fn start_video(output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
    let factory = VIDEO_BACKEND_FACTORY
        .get()
        .ok_or_else(|| anyhow::anyhow!("no video backend registered for this platform"))?;
    factory.start(output_path)
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn window_target_wire_is_exact_and_validated() {
        let target = WindowVideoTarget {
            pid: 12,
            window_id: 34,
        };
        let value = serde_json::json!({"kind":"window", "pid":12, "window_id":34});
        assert_eq!(serde_json::to_value(&target).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<WindowVideoTarget>(value).unwrap(),
            target
        );
        for value in [
            serde_json::json!({"kind":"display","pid":12,"window_id":34}),
            serde_json::json!({"kind":"window","pid":0,"window_id":34}),
            serde_json::json!({"kind":"window","pid":12,"window_id":0}),
            serde_json::json!({"kind":"window","pid":2147483648u64,"window_id":34}),
            serde_json::json!({"kind":"window","pid":12}),
            serde_json::json!({"kind":"window","pid":12,"window_id":34,"scope":"desktop"}),
        ] {
            assert!(serde_json::from_value::<WindowVideoTarget>(value).is_err());
        }
    }

    struct LegacyFactory;
    impl VideoBackendFactory for LegacyFactory {
        fn start(&self, _: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
            panic!("window requests must never start a display backend")
        }
    }

    #[test]
    fn legacy_factory_refuses_window_without_display_fallback() {
        let result = LegacyFactory.prepare_window(&WindowVideoTarget {
            pid: 12,
            window_id: 34,
        });
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .starts_with("window_recording_unsupported:"));
    }
}
