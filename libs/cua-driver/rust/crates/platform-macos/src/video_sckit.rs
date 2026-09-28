//! Native ScreenCaptureKit video backend (macOS).
//!
//! Replaces the ffmpeg subprocess pipeline with an in-process SCStream +
//! SCRecordingOutput. The key win is TCC: ScreenCaptureKit runs in the
//! same process as cua-driver, so it inherits the daemon's Screen
//! Recording grant. No per-binary subprocess gotcha, no second prompt,
//! no fast-fail-on-hang heuristic.
//!
//! Requires macOS 15.0+ (SCRecordingOutput introduced in macOS 15). The
//! Swift impl this is modelled on lives at
//! `libs/cua-driver/swift/Sources/CuaDriverCore/Recording/VideoRecorder.swift`,
//! though that version composes SCStream + AVAssetWriter manually so it
//! also runs on macOS 14. We use SCRecordingOutput here because the
//! Rust binding doesn't expose AVAssetWriter and macOS 15 is already
//! widespread enough that requiring it is acceptable for the Rust port.
//!
//! Lifecycle:
//!   1. `start(path)` resolves the main display, builds a 30fps full-display
//!      SCStream config + SCRecordingOutput pointing at the mp4 path,
//!      attaches the recording output, calls `start_capture()`.
//!   2. Caller stays alive while recording.
//!   3. `stop()` calls `stop_capture()`, then waits (bounded by
//!      [`FINISH_TIMEOUT`]) for SCRecordingOutput's delegate to report that
//!      the mp4 is written, and returns the elapsed-time metadata.
//!
//! `stop_capture()` returning does not mean the file is complete:
//! SCRecordingOutput finishes writing asynchronously and signals it through
//! `recordingOutputDidFinishRecording:` (or `recordingOutput:didFailWithError:`).
//! Reporting the video as finalized before that callback let an immediate
//! reader see a 0-byte file for short recordings.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cua_driver_core::video::{VideoBackend, VideoBackendFactory, VideoMetadata};

use screencapturekit::prelude::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration,
};
use screencapturekit::recording_output::{
    SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputDelegate, SCRecordingOutputFileType,
};

/// Upper bound on how long `stop()` waits for SCRecordingOutput to finish
/// writing the mp4 after capture stops. Finalizing normally takes tens of
/// milliseconds; the bound keeps a lost callback from hanging `stop_recording`.
pub const FINISH_TIMEOUT: Duration = Duration::from_secs(15);

/// Terminal outcome reported by the SCRecordingOutput delegate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FinishOutcome {
    Finished,
    Failed(String),
}

/// Why waiting for the recording to finish did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishWaitError {
    /// The delegate reported `recordingOutput:didFailWithError:`.
    Failed(String),
    /// Neither delegate callback arrived within the timeout.
    TimedOut(Duration),
}

impl std::fmt::Display for FinishWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(error) => write!(f, "SCRecordingOutput failed to write the recording: {error}"),
            Self::TimedOut(timeout) => write!(
                f,
                "SCRecordingOutput did not finish writing the recording within {} ms after capture stopped",
                timeout.as_millis()
            ),
        }
    }
}

/// One-shot latch the delegate sets when the recording output reaches a
/// terminal state. The first terminal event wins; later events are ignored.
#[derive(Debug, Default)]
pub struct RecordingFinishSignal {
    outcome: Mutex<Option<FinishOutcome>>,
    changed: Condvar,
}

impl RecordingFinishSignal {
    fn settle(&self, outcome: FinishOutcome) {
        let mut slot = self.outcome.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(outcome);
            self.changed.notify_all();
        }
    }

    pub fn mark_finished(&self) {
        self.settle(FinishOutcome::Finished);
    }

    pub fn mark_failed(&self, error: String) {
        self.settle(FinishOutcome::Failed(error));
    }

    /// Blocks until the recording output reports a terminal state or
    /// `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> Result<(), FinishWaitError> {
        let slot = self.outcome.lock().unwrap_or_else(|e| e.into_inner());
        let (slot, _) = self
            .changed
            .wait_timeout_while(slot, timeout, |outcome| outcome.is_none())
            .unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some(FinishOutcome::Finished) => Ok(()),
            Some(FinishOutcome::Failed(error)) => Err(FinishWaitError::Failed(error.clone())),
            None => Err(FinishWaitError::TimedOut(timeout)),
        }
    }
}

/// Delegate that forwards SCRecordingOutput's terminal callbacks into a
/// [`RecordingFinishSignal`].
struct FinishDelegate(Arc<RecordingFinishSignal>);

impl SCRecordingOutputDelegate for FinishDelegate {
    fn recording_did_fail(&self, error: String) {
        self.0.mark_failed(error);
    }

    fn recording_did_finish(&self) {
        self.0.mark_finished();
    }
}

pub struct SckitVideoBackendFactory;

impl VideoBackendFactory for SckitVideoBackendFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        SckitVideoBackend::start(output_path).map(|b| Box::new(b) as Box<dyn VideoBackend>)
    }
}

pub struct SckitVideoBackend {
    stream: SCStream,
    // SCStream's add_recording_output is non-owning — Apple's API requires
    // the SCRecordingOutput stay alive for the stream's lifetime, so we
    // keep it parked here. Dropping it before the finish callback aborts
    // the encode mid-file and unregisters the delegate.
    _recording: SCRecordingOutput,
    finish: Arc<RecordingFinishSignal>,
    output_path: std::path::PathBuf,
    started_at: Instant,
}

impl SckitVideoBackend {
    fn start(output_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!(
                    "failed to create recording output directory {}: {e}",
                    parent.display()
                )
            })?;
        }
        // SCRecordingOutput appends-or-fails on an existing file; match the
        // Swift impl by clearing any stale recording.mp4 from a prior run.
        match std::fs::remove_file(output_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                anyhow::bail!(
                    "failed to remove stale recording file {}: {e}",
                    output_path.display()
                );
            }
        }

        let content = SCShareableContent::get()
            .map_err(|e| anyhow::anyhow!("SCShareableContent::get failed: {e}"))?;
        let displays = content.displays();
        let display = displays
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no displays available for ScreenCaptureKit"))?;

        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();

        // Match the Swift recorder's pixel resolution + 30fps target. The
        // display's reported width/height are in pixels (already
        // backing-scale-multiplied) on SCDisplay, so passing them through
        // gives a native-resolution capture.
        let pixel_width = display.width();
        let pixel_height = display.height();
        let frame_interval = screencapturekit::cm::CMTime::new(1, 30);
        let config = SCStreamConfiguration::new()
            .with_width(pixel_width)
            .with_height(pixel_height)
            .with_minimum_frame_interval(&frame_interval)
            .with_shows_cursor(true);

        let rec_config = SCRecordingOutputConfiguration::new()
            .with_output_url(output_path)
            .with_video_codec(SCRecordingOutputCodec::H264)
            .with_output_file_type(SCRecordingOutputFileType::MP4);

        let finish = Arc::new(RecordingFinishSignal::default());
        let recording =
            SCRecordingOutput::new_with_delegate(&rec_config, FinishDelegate(finish.clone()))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                "SCRecordingOutput::new_with_delegate returned nil — macOS 15.0+ is required for \
                 native ScreenCaptureKit video; older macOS needs to use the ffmpeg \
                 backend (currently disabled on macOS)."
            )
                })?;

        let stream = SCStream::new(&filter, &config);
        stream
            .add_recording_output(&recording)
            .map_err(|e| anyhow::anyhow!("SCStream::add_recording_output failed: {e}"))?;
        stream
            .start_capture()
            .map_err(|e| anyhow::anyhow!("SCStream::start_capture failed: {e}"))?;

        tracing::info!(
            target: "recording",
            path = %output_path.display(),
            width = pixel_width,
            height = pixel_height,
            "sckit video capture started"
        );

        Ok(Self {
            stream,
            _recording: recording,
            finish,
            output_path: output_path.to_path_buf(),
            started_at: Instant::now(),
        })
    }
}

impl VideoBackend for SckitVideoBackend {
    fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();
        // stop_capture() only stops the stream; SCRecordingOutput writes the
        // rest of the mp4 asynchronously. Errors here mean the file may be
        // unplayable — surface as `finalized: false`.
        if let Err(error) = self.stream.stop_capture() {
            tracing::warn!(
                target: "recording",
                %error,
                "SCStream::stop_capture failed; recording.mp4 may be incomplete"
            );
            return Ok(VideoMetadata {
                path: self.output_path,
                duration_ms: elapsed.as_millis() as u64,
                finalized: false,
            });
        }
        let wait_started = Instant::now();
        self.finish
            .wait(FINISH_TIMEOUT)
            .map_err(|error| anyhow::anyhow!("{error} ({})", self.output_path.display()))?;
        tracing::debug!(
            target: "recording",
            path = %self.output_path.display(),
            finish_wait_ms = wait_started.elapsed().as_millis() as u64,
            "sckit recording finished writing"
        );
        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn finish_before_wait_returns_immediately() {
        let signal = RecordingFinishSignal::default();
        signal.mark_finished();
        assert_eq!(signal.wait(Duration::from_millis(1)), Ok(()));
    }

    #[test]
    fn wait_blocks_until_the_delegate_reports_finish() {
        let signal = Arc::new(RecordingFinishSignal::default());
        let delegate = FinishDelegate(signal.clone());
        let started = Instant::now();
        let finisher = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            delegate.recording_did_finish();
        });
        assert_eq!(signal.wait(Duration::from_secs(10)), Ok(()));
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "wait returned before the finish callback"
        );
        finisher.join().unwrap();
    }

    #[test]
    fn delegate_failure_is_reported_as_an_error() {
        let signal = Arc::new(RecordingFinishSignal::default());
        FinishDelegate(signal.clone()).recording_did_fail("disk full".into());
        let error = signal.wait(Duration::from_secs(1)).unwrap_err();
        assert_eq!(error, FinishWaitError::Failed("disk full".into()));
        assert!(error.to_string().contains("disk full"));
    }

    #[test]
    fn first_terminal_event_wins() {
        let signal = RecordingFinishSignal::default();
        signal.mark_finished();
        signal.mark_failed("late".into());
        assert_eq!(signal.wait(Duration::from_millis(1)), Ok(()));
    }

    #[test]
    fn missing_callback_times_out_with_a_clear_error() {
        let signal = RecordingFinishSignal::default();
        let timeout = Duration::from_millis(50);
        let started = Instant::now();
        let error = signal.wait(timeout).unwrap_err();
        assert!(started.elapsed() >= timeout);
        assert_eq!(error, FinishWaitError::TimedOut(timeout));
        assert!(error.to_string().contains("did not finish writing"));
    }

    /// Live regression for D-MAC-10: a sub-second recording must be
    /// non-empty the moment `stop()` reports it finalized.
    #[test]
    #[ignore = "requires Screen Recording permission and a display; run with --ignored on a TCC-authorized macOS host"]
    fn short_recording_is_written_when_stop_returns() {
        let dir = std::env::temp_dir().join(format!("cua-sck-finish-{}", std::process::id()));
        let path = dir.join("recording.mp4");
        for _ in 0..5 {
            let backend = SckitVideoBackendFactory
                .start(&path)
                .expect("start sckit recording");
            thread::sleep(Duration::from_millis(120));
            let meta = backend.stop().expect("stop sckit recording");
            assert!(meta.finalized);
            let len = std::fs::metadata(&path).expect("recording exists").len();
            assert!(len > 0, "recording is empty when stop() returned");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
