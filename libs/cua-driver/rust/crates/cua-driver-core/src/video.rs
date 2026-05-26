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
use std::sync::OnceLock;

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
}

/// Spawns a fresh `VideoBackend` writing to `output_path`. Registered
/// once at startup via `set_video_backend_factory`.
pub trait VideoBackendFactory: Send + Sync {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>>;
}

static VIDEO_BACKEND_FACTORY: OnceLock<Box<dyn VideoBackendFactory>> = OnceLock::new();

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
