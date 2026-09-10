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
//!   3. `stop()` calls `stop_capture()` (which finalises the mp4 moov
//!      atom) and returns the elapsed-time metadata.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cua_driver_core::video::{
    PreparedWindowVideo, VideoBackend, VideoBackendFactory, VideoMetadata, WindowVideoInfo,
    WindowVideoObserver, WindowVideoStatus, WindowVideoTarget,
};

use screencapturekit::prelude::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration,
};
use screencapturekit::recording_output::{
    SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputDelegate, SCRecordingOutputFileType,
};

pub struct SckitVideoBackendFactory;

impl VideoBackendFactory for SckitVideoBackendFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        SckitVideoBackend::start(output_path).map(|b| Box::new(b) as Box<dyn VideoBackend>)
    }

    fn prepare_window(
        &self,
        target: &WindowVideoTarget,
    ) -> anyhow::Result<Box<dyn PreparedWindowVideo>> {
        anyhow::ensure!(
            objc2::runtime::AnyClass::get("SCRecordingOutput").is_some(),
            "window_recording_unsupported: macOS 15 or newer is required"
        );
        let plan = WindowPlan::resolve(target)?;
        Ok(Box::new(PreparedSckitWindow {
            info: plan.info,
            fingerprint: plan.fingerprint,
        }))
    }
}

pub struct SckitVideoBackend {
    stream: SCStream,
    // SCStream's add_recording_output is non-owning — Apple's API requires
    // the SCRecordingOutput stay alive for the stream's lifetime, so we
    // keep it parked here. Dropping it before stop_capture aborts the
    // encode mid-file.
    _recording: SCRecordingOutput,
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

        let recording = SCRecordingOutput::new(&rec_config).ok_or_else(|| {
            anyhow::anyhow!(
                "SCRecordingOutput::new returned nil — macOS 15.0+ is required for \
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
            output_path: output_path.to_path_buf(),
            started_at: Instant::now(),
        })
    }
}

impl VideoBackend for SckitVideoBackend {
    fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();
        // SCStream::stop_capture finalises the mp4 moov atom synchronously
        // on the recording output before returning. Errors here mean the
        // file may be unplayable — surface as `finalized: false`.
        let finalized = self.stream.stop_capture().is_ok();
        if !finalized {
            tracing::warn!(
                target: "recording",
                "SCStream::stop_capture failed; recording.mp4 may be incomplete"
            );
        }
        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized,
        })
    }
}

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(10);
const CONTENT_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_millis(250);

// Unlike screenshot identity, this deliberately excludes window position.
#[derive(Debug, Clone, PartialEq)]
struct WindowFingerprint {
    pid: i32,
    layer: i32,
    native_width: f64,
    native_height: f64,
    width: f64,
    height: f64,
    content_width: f64,
    content_height: f64,
    scale: f64,
}

impl WindowFingerprint {
    fn new(
        pid: i32,
        layer: i32,
        native_size: (f64, f64),
        frame: screencapturekit::cg::CGRect,
        content: screencapturekit::cg::CGRect,
        scale: f64,
    ) -> Self {
        Self {
            pid,
            layer,
            native_width: native_size.0,
            native_height: native_size.1,
            width: frame.size.width,
            height: frame.size.height,
            content_width: content.size.width,
            content_height: content.size.height,
            scale,
        }
    }

    fn changed_reason(&self, current: &Self) -> Option<&'static str> {
        if self.pid != current.pid || self.layer != current.layer {
            Some("window_identity_changed")
        } else if self.scale != current.scale {
            Some("window_scale_changed")
        } else if self != current {
            Some("window_resized")
        } else {
            None
        }
    }
}

fn check_window_recheck(
    expected: &WindowFingerprint,
    resolved: anyhow::Result<WindowFingerprint>,
) -> Result<(), String> {
    let current = resolved.map_err(|error| error.to_string())?;
    match expected.changed_reason(&current) {
        Some(reason) => Err(reason.to_owned()),
        None => Ok(()),
    }
}

fn even_pixel_dimension(points: f64, scale: f64) -> anyhow::Result<u32> {
    let pixels = (points * scale).ceil();
    anyhow::ensure!(
        points.is_finite()
            && points > 0.0
            && scale.is_finite()
            && scale > 0.0
            && pixels > 0.0
            && pixels <= f64::from(u32::MAX - 1),
        "window_geometry_invalid"
    );
    // Round upwards so H.264's even dimensions never trim edge content.
    Ok((pixels as u32 + 1) & !1)
}

fn wait_future<F: std::future::Future>(future: F, timeout: Duration) -> anyhow::Result<F::Output> {
    struct WakeThread(std::thread::Thread);
    impl std::task::Wake for WakeThread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(WakeThread(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    let deadline = Instant::now() + timeout;
    loop {
        if let std::task::Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return Ok(result);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "window_health_timeout");
        std::thread::park_timeout(remaining);
    }
}

struct WindowPlan {
    info: WindowVideoInfo,
    fingerprint: WindowFingerprint,
    filter: SCContentFilter,
}

impl WindowPlan {
    fn resolve(target: &WindowVideoTarget) -> anyhow::Result<Self> {
        target.validate()?;
        let window_id = u32::try_from(target.window_id).map_err(|_| {
            anyhow::anyhow!("invalid_recording_target: window_id exceeds CGWindowID")
        })?;
        let native = crate::windows::window_info_by_id(window_id)
            .ok_or_else(|| anyhow::anyhow!("window_closed"))?;
        anyhow::ensure!(native.pid == target.pid, "window_owner_changed");
        anyhow::ensure!(native.is_on_screen, "window_not_visible");
        let content = wait_future(
            screencapturekit::async_api::AsyncSCShareableContent::get(),
            CONTENT_TIMEOUT,
        )?
        .map_err(|_| anyhow::anyhow!("window_not_shareable"))?;
        let window = content
            .windows()
            .into_iter()
            .find(|w| w.window_id() == window_id)
            .ok_or_else(|| anyhow::anyhow!("window_not_shareable"))?;
        let owner = window
            .owning_application()
            .ok_or_else(|| anyhow::anyhow!("window_owner_changed"))?;
        anyhow::ensure!(owner.process_id() == target.pid, "window_owner_changed");
        let frame = window.frame();
        anyhow::ensure!(
            window.window_layer() == native.layer,
            "window_identity_changed"
        );
        // WindowServer and ScreenCaptureKit can expose different bounds for the
        // same window. Keep both sizes in the fingerprint, but compare each
        // only against later readings from its own API.
        anyhow::ensure!(
            [
                native.bounds.width,
                native.bounds.height,
                frame.size.width,
                frame.size.height
            ]
            .into_iter()
            .all(|dimension| dimension.is_finite() && dimension > 0.0),
            "window_geometry_invalid"
        );
        let filter = SCContentFilter::create().with_window(&window).build();
        let rect = filter.content_rect();
        let scale = f64::from(filter.point_pixel_scale());
        let width = even_pixel_dimension(rect.size.width, scale)?;
        let height = even_pixel_dimension(rect.size.height, scale)?;
        // Recheck WindowServer after the asynchronous enumeration, including visibility.
        let latest = crate::windows::window_info_by_id(window_id)
            .ok_or_else(|| anyhow::anyhow!("window_closed"))?;
        anyhow::ensure!(latest.pid == target.pid, "window_owner_changed");
        anyhow::ensure!(latest.is_on_screen, "window_not_visible");
        anyhow::ensure!(latest.layer == native.layer, "window_identity_changed");
        anyhow::ensure!(
            latest.bounds.width == native.bounds.width
                && latest.bounds.height == native.bounds.height,
            "window_resized"
        );
        Ok(Self {
            info: WindowVideoInfo {
                target: target.clone(),
                width,
                height,
                backend: "screencapturekit_window".into(),
            },
            fingerprint: WindowFingerprint::new(
                target.pid,
                native.layer,
                (native.bounds.width, native.bounds.height),
                frame,
                rect,
                scale,
            ),
            filter,
        })
    }
}

struct PreparedSckitWindow {
    info: WindowVideoInfo,
    fingerprint: WindowFingerprint,
}

#[derive(Debug)]
enum WindowEvent {
    Started,
    Finished,
    Failed,
    StreamStopped,
    Stop,
}

fn wait_recording_start(
    events: &mpsc::Receiver<WindowEvent>,
    timeout: Duration,
) -> Result<(), &'static str> {
    match events.recv_timeout(timeout) {
        Ok(WindowEvent::Started) => Ok(()),
        Ok(WindowEvent::Failed) => Err("window_recording_failed"),
        Err(mpsc::RecvTimeoutError::Timeout) => Err("window_recording_start_timeout"),
        _ => Err("window_recording_start_failed"),
    }
}

fn wait_recording_finish(
    events: &mpsc::Receiver<WindowEvent>,
    timeout: Duration,
) -> Result<(), &'static str> {
    let deadline = Instant::now() + timeout;
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(WindowEvent::Finished) => return Ok(()),
            Ok(WindowEvent::Failed) => return Err("window_recording_failed"),
            Ok(_) => {}
            Err(_) => return Err("window_recording_finalize_timeout"),
        }
    }
}

struct RecordingDelegate(mpsc::Sender<WindowEvent>);
impl SCRecordingOutputDelegate for RecordingDelegate {
    fn recording_did_start(&self) {
        let _ = self.0.send(WindowEvent::Started);
    }
    fn recording_did_finish(&self) {
        let _ = self.0.send(WindowEvent::Finished);
    }
    fn recording_did_fail(&self, _error: String) {
        let _ = self.0.send(WindowEvent::Failed);
    }
}

struct StreamDelegate(mpsc::Sender<WindowEvent>);
impl screencapturekit::stream::delegate_trait::SCStreamDelegateTrait for StreamDelegate {
    fn did_stop_with_error(&self, _error: screencapturekit::error::SCError) {
        let _ = self.0.send(WindowEvent::StreamStopped);
    }
    fn stream_did_stop(&self, _error: Option<String>) {
        let _ = self.0.send(WindowEvent::StreamStopped);
    }
    fn stream_did_become_inactive(&self) {
        let _ = self.0.send(WindowEvent::StreamStopped);
    }
}

struct WindowState {
    status: Mutex<WindowVideoStatus>,
    observer: WindowVideoObserver,
}

impl WindowState {
    fn publish(&self, change: impl FnOnce(&mut WindowVideoStatus)) {
        let status = {
            let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
            change(&mut status);
            status.clone()
        };
        (self.observer)(status);
    }
    fn snapshot(&self) -> WindowVideoStatus {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl PreparedWindowVideo for PreparedSckitWindow {
    fn info(&self) -> WindowVideoInfo {
        self.info.clone()
    }

    fn start(
        self: Box<Self>,
        path: &Path,
        observer: WindowVideoObserver,
    ) -> anyhow::Result<Box<dyn VideoBackend>> {
        let plan = WindowPlan::resolve(&self.info.target)?;
        if let Some(reason) = self.fingerprint.changed_reason(&plan.fingerprint) {
            anyhow::bail!("{reason}");
        }
        // Core reserves the directory; the backend never removes or replaces a video.
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => anyhow::bail!("recording_output_exists"),
        }
        let state = Arc::new(WindowState {
            status: Mutex::new(WindowVideoStatus {
                info: self.info.clone(),
                active: false,
                finalized: false,
                duration_ms: 0,
                termination_reason: None,
                error: None,
            }),
            observer,
        });
        let (events_tx, events_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_state = state.clone();
        let worker_events = events_tx.clone();
        let output_path = path.to_path_buf();
        let worker_path = output_path.clone();
        let worker = std::thread::Builder::new()
            .name("window-video".into())
            .spawn(move || {
                run_window_worker(
                    plan,
                    worker_path,
                    worker_state,
                    worker_events,
                    events_rx,
                    ready_tx,
                );
            })?;
        let backend = WindowVideoBackend {
            output_path,
            state,
            events: events_tx,
            worker: Some(worker),
        };
        // Native transport calls in screencapturekit 6.0.1 are synchronous and have
        // no cancellation API. Callback and content waits below are bounded, but
        // a framework transport hang can still delay startup or joining this worker.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(backend)),
            Ok(Err(reason)) => {
                drop(backend);
                anyhow::bail!("{reason}")
            }
            Err(_) => {
                drop(backend);
                anyhow::bail!("window_recording_worker_failed")
            }
        }
    }
}

struct WindowVideoBackend {
    output_path: PathBuf,
    state: Arc<WindowState>,
    events: mpsc::Sender<WindowEvent>,
    worker: Option<JoinHandle<()>>,
}

impl WindowVideoBackend {
    fn shutdown(&mut self) {
        let _ = self.events.send(WindowEvent::Stop);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                self.state.publish(|status| {
                    status.active = false;
                    status.finalized = false;
                    status.error = Some("window_recording_worker_failed".into());
                    status.termination_reason = Some("window_recording_worker_failed".into());
                });
            }
        }
    }
}

impl Drop for WindowVideoBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl VideoBackend for WindowVideoBackend {
    fn stop(mut self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        self.shutdown();
        let status = self.state.snapshot();
        Ok(VideoMetadata {
            path: self.output_path.clone(),
            duration_ms: status.duration_ms,
            finalized: status.finalized,
        })
    }
    fn window_status(&self) -> Option<WindowVideoStatus> {
        Some(self.state.snapshot())
    }
}

fn run_window_worker(
    plan: WindowPlan,
    path: PathBuf,
    state: Arc<WindowState>,
    events_tx: mpsc::Sender<WindowEvent>,
    events: mpsc::Receiver<WindowEvent>,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    let config = SCStreamConfiguration::new()
        .with_width(plan.info.width)
        .with_height(plan.info.height)
        .with_minimum_frame_interval(&screencapturekit::cm::CMTime::new(1, 30))
        .with_shows_cursor(false)
        .with_captures_audio(false)
        .with_captures_microphone(false);
    let recording_config = SCRecordingOutputConfiguration::new()
        .with_output_url(&path)
        .with_video_codec(SCRecordingOutputCodec::H264)
        .with_output_file_type(SCRecordingOutputFileType::MP4);
    let Some(recording) = SCRecordingOutput::new_with_delegate(
        &recording_config,
        RecordingDelegate(events_tx.clone()),
    ) else {
        let _ = ready.send(Err("window_recording_unsupported".into()));
        return;
    };
    let stream = SCStream::new_with_delegate(&plan.filter, &config, StreamDelegate(events_tx));
    let mut attached = false;
    let mut capture_attempted = false;
    let startup = (|| -> Result<(), String> {
        stream
            .add_recording_output(&recording)
            .map_err(|_| "window_recording_attach_failed".to_owned())?;
        attached = true;
        // Check the target again immediately before asking the native stream to start.
        check_window_recheck(
            &plan.fingerprint,
            WindowPlan::resolve(&plan.info.target).map(|current| current.fingerprint),
        )?;
        capture_attempted = true;
        stream
            .start_capture()
            .map_err(|_| "window_recording_start_failed".to_owned())?;
        wait_recording_start(&events, CALLBACK_TIMEOUT).map_err(str::to_owned)
    })();
    if let Err(reason) = startup {
        state.publish(|status| {
            status.termination_reason = Some(reason.clone());
            status.error = Some(reason.clone());
        });
        if capture_attempted {
            finish_window_stream(&stream, &recording, &events, &state, None);
        } else if attached {
            let _ = stream.remove_recording_output(&recording);
        }
        let _ = ready.send(Err(reason));
        return;
    }
    let started = Instant::now();
    state.publish(|status| {
        status.active = true;
    });
    if ready.send(Ok(())).is_err() {
        state.publish(|status| {
            status.termination_reason = Some("recording_dropped".into());
        });
        finish_window_stream(&stream, &recording, &events, &state, Some(started));
        return;
    }
    let mut next_health = Instant::now() + HEALTH_INTERVAL;
    let (reason, error, already_finished) = loop {
        match events.recv_timeout(next_health.saturating_duration_since(Instant::now())) {
            Ok(WindowEvent::Stop) => break ("stopped".to_owned(), None, false),
            Ok(WindowEvent::Failed) => {
                break (
                    "window_recording_failed".into(),
                    Some("window_recording_failed".into()),
                    false,
                )
            }
            Ok(WindowEvent::StreamStopped) => {
                break (
                    "window_stream_stopped".into(),
                    Some("window_stream_stopped".into()),
                    false,
                )
            }
            Ok(WindowEvent::Finished) => break ("window_recording_finished".into(), None, true),
            Ok(WindowEvent::Started) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break ("recording_dropped".into(), None, false)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        match WindowPlan::resolve(&plan.info.target) {
            Ok(current) => {
                if let Some(reason) = plan.fingerprint.changed_reason(&current.fingerprint) {
                    break (reason.into(), None, false);
                }
            }
            Err(error) => break (error.to_string(), None, false),
        }
        state.publish(|status| {
            status.duration_ms = started.elapsed().as_millis() as u64;
        });
        next_health = Instant::now() + HEALTH_INTERVAL;
    };
    state.publish(|status| {
        status.active = false;
        status.duration_ms = started.elapsed().as_millis() as u64;
        status.termination_reason = Some(reason);
        status.error = error;
        status.finalized = already_finished;
    });
    finish_window_stream(&stream, &recording, &events, &state, Some(started));
}

fn finish_window_stream(
    stream: &SCStream,
    recording: &SCRecordingOutput,
    events: &mpsc::Receiver<WindowEvent>,
    state: &WindowState,
    started: Option<Instant>,
) {
    let transport_ok = stream.stop_capture().is_ok();
    let status = state.snapshot();
    let mut finalized = status.finalized;
    let mut failure = (status.error.as_deref() == Some("window_recording_failed"))
        .then_some("window_recording_failed");
    if !finalized && failure.is_none() {
        match wait_recording_finish(events, CALLBACK_TIMEOUT) {
            Ok(()) => finalized = true,
            Err(reason) => failure = Some(reason),
        }
    }
    if !transport_ok && failure.is_none() && !finalized {
        failure = Some("window_recording_stop_failed");
    }
    // Keep the output alive through its finish callback, then detach before both
    // wrappers drop. No native resource is stored in the returned status handle.
    let _ = stream.remove_recording_output(recording);
    state.publish(|status| {
        status.active = false;
        status.finalized = finalized;
        if let Some(started) = started {
            if status.duration_ms == 0 {
                status.duration_ms = started.elapsed().as_millis() as u64;
            }
        }
        if let Some(failure) = failure {
            status.error.get_or_insert_with(|| failure.into());
        }
    });
}

#[cfg(test)]
mod window_video_tests {
    use super::*;

    fn fingerprint() -> WindowFingerprint {
        WindowFingerprint {
            pid: 42,
            layer: 0,
            native_width: 803.0,
            native_height: 603.0,
            width: 801.0,
            height: 601.0,
            content_width: 801.0,
            content_height: 601.0,
            scale: 2.0,
        }
    }

    #[test]
    fn encoder_dimensions_round_up_without_trimming() {
        assert_eq!(even_pixel_dimension(801.0, 1.0).unwrap(), 802);
        assert_eq!(even_pixel_dimension(801.0, 2.0).unwrap(), 1602);
        assert_eq!(even_pixel_dimension(800.25, 1.0).unwrap(), 802);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::from(u32::MAX)] {
            assert!(even_pixel_dimension(bad, 1.0).is_err());
            assert!(even_pixel_dimension(1.0, bad).is_err());
        }
    }

    #[test]
    fn fixed_geometry_classifies_owner_scale_and_resize() {
        let original = fingerprint();
        assert_eq!(original.changed_reason(&fingerprint()), None);
        let mut current = original.clone();
        current.pid += 1;
        assert_eq!(
            original.changed_reason(&current),
            Some("window_identity_changed")
        );
        current = original.clone();
        current.layer += 1;
        assert_eq!(
            original.changed_reason(&current),
            Some("window_identity_changed")
        );
        current = original.clone();
        current.scale = 1.0;
        assert_eq!(
            original.changed_reason(&current),
            Some("window_scale_changed")
        );
        current = original.clone();
        current.native_width += 1.0;
        assert_eq!(original.changed_reason(&current), Some("window_resized"));
        current = original.clone();
        current.native_height += 1.0;
        assert_eq!(original.changed_reason(&current), Some("window_resized"));
        current = original.clone();
        current.width += 1.0;
        assert_eq!(original.changed_reason(&current), Some("window_resized"));
        current = original.clone();
        current.content_height += 1.0;
        assert_eq!(original.changed_reason(&current), Some("window_resized"));
    }

    #[test]
    fn origin_movement_does_not_change_capture_identity() {
        use screencapturekit::cg::{CGPoint, CGRect, CGSize};
        let frame = CGRect {
            origin: CGPoint { x: 10.0, y: 20.0 },
            size: CGSize {
                width: 801.0,
                height: 601.0,
            },
        };
        let moved = CGRect {
            origin: CGPoint {
                x: -200.0,
                y: 900.0,
            },
            ..frame
        };
        let original = WindowFingerprint::new(42, 0, (803.0, 603.0), frame, frame, 2.0);
        let current = WindowFingerprint::new(42, 0, (803.0, 603.0), moved, moved, 2.0);
        assert_eq!(original.changed_reason(&current), None);
    }

    #[test]
    fn actual_recording_callback_is_required_for_startup() {
        let (sender, events) = mpsc::channel();
        assert_eq!(
            wait_recording_start(&events, Duration::ZERO),
            Err("window_recording_start_timeout")
        );
        sender.send(WindowEvent::Started).unwrap();
        assert_eq!(wait_recording_start(&events, Duration::ZERO), Ok(()));
        sender.send(WindowEvent::Failed).unwrap();
        assert_eq!(
            wait_recording_start(&events, Duration::ZERO),
            Err("window_recording_failed")
        );
    }

    #[test]
    fn only_recording_finish_callback_proves_finalization() {
        let (sender, events) = mpsc::channel();
        sender.send(WindowEvent::StreamStopped).unwrap();
        assert_eq!(
            wait_recording_finish(&events, Duration::ZERO),
            Err("window_recording_finalize_timeout")
        );
        sender.send(WindowEvent::Finished).unwrap();
        assert_eq!(wait_recording_finish(&events, Duration::ZERO), Ok(()));
        sender.send(WindowEvent::Failed).unwrap();
        assert_eq!(
            wait_recording_finish(&events, Duration::ZERO),
            Err("window_recording_failed")
        );
    }

    #[test]
    fn content_future_deadline_is_bounded() {
        let result = wait_future(std::future::pending::<()>(), Duration::from_millis(1));
        assert_eq!(result.unwrap_err().to_string(), "window_health_timeout");
        assert_eq!(
            wait_future(std::future::ready(42), Duration::ZERO).unwrap(),
            42
        );
    }

    #[test]
    fn prestart_recheck_preserves_classified_resolution_failures() {
        for reason in [
            "window_closed",
            "window_not_visible",
            "window_not_shareable",
            "window_owner_changed",
            "window_health_timeout",
        ] {
            assert_eq!(
                check_window_recheck(&fingerprint(), Err(anyhow::anyhow!(reason))),
                Err(reason.to_owned())
            );
        }
        assert!(check_window_recheck(&fingerprint(), Ok(fingerprint())).is_ok());
    }
}
