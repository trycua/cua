//! Native Wayland full-desktop video capture.
//!
//! One factory, three compositor routes (see [`WaylandVideoRoute`]):
//!
//! - GNOME's attested Shell helper supplies compositor-owned PNG frames that
//!   are encoded into MP4 through FFmpeg; no consent dialog is involved.
//! - wlroots compositors advertise `zwlr_screencopy_manager_v1`, which
//!   `wf-recorder` consumes directly.
//! - Everything else (KDE/KWin, portal-only desktops) records through the
//!   xdg-desktop-portal ScreenCast: the user grants a monitor once, the
//!   PipeWire stream is paced to a constant frame rate, and FFmpeg encodes
//!   raw frames into the same MP4 artifact. This route needs the
//!   `portal-capture` feature; builds without it report the limitation.

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use cua_driver_core::video::{VideoBackend, VideoBackendFactory, VideoMetadata};
use cua_driver_core::video_ffmpeg::find_ffmpeg;

pub struct WaylandVideoBackendFactory;

impl VideoBackendFactory for WaylandVideoBackendFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        let first_frame = crate::wayland::shell_helper::trusted_screenshot_display();
        let wlr_screencopy = crate::wayland::probe_managers()
            .ok()
            .map(|managers| managers.screencopy);
        match select_route(first_frame.is_some(), wlr_screencopy) {
            WaylandVideoRoute::GnomeShell => GnomeShellVideoBackend::start(
                output_path,
                first_frame.expect("GnomeShell route requires a trusted first frame"),
            )
            .map(|backend| Box::new(backend) as Box<dyn VideoBackend>),
            WaylandVideoRoute::WfRecorder => WfRecorderVideoBackend::start(output_path)
                .map(|backend| Box::new(backend) as Box<dyn VideoBackend>),
            WaylandVideoRoute::PortalScreencast => start_portal_video(output_path),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaylandVideoRoute {
    GnomeShell,
    WfRecorder,
    PortalScreencast,
}

/// `wlr_screencopy` is `None` when the registry could not be probed; the
/// wf-recorder route then keeps its own startup diagnostics as the report.
fn select_route(trusted_gnome_frames: bool, wlr_screencopy: Option<bool>) -> WaylandVideoRoute {
    if trusted_gnome_frames {
        WaylandVideoRoute::GnomeShell
    } else if wlr_screencopy == Some(false) {
        WaylandVideoRoute::PortalScreencast
    } else {
        WaylandVideoRoute::WfRecorder
    }
}

#[cfg(not(feature = "portal-capture"))]
fn start_portal_video(_output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
    anyhow::bail!(
        "this compositor does not expose wlr-screencopy, and this cua-driver build was compiled \
         without the `portal-capture` feature, so xdg-desktop-portal ScreenCast video is \
         unavailable. Use a build with `--features portal-capture` (the Nix package enables it) \
         to record on KDE/KWin and other portal-only compositors."
    )
}

#[cfg(feature = "portal-capture")]
fn start_portal_video(output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
    portal_video::PortalVideoBackend::start(output_path)
        .map(|backend| Box::new(backend) as Box<dyn VideoBackend>)
}

fn require_ffmpeg(route: &str) -> anyhow::Result<PathBuf> {
    find_ffmpeg().ok_or_else(|| anyhow::anyhow!("ffmpeg is required for {route} video capture"))
}

fn prepare_output_dir(output_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Collect the tail of a child's stderr without letting the pipe fill up.
fn drain_stderr_tail(child: &mut Child) -> Option<std::thread::JoinHandle<Vec<u8>>> {
    child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = stderr.read_to_end(&mut output);
            if output.len() > 4096 {
                let excess = output.len() - 4096;
                output.drain(..excess);
            }
            output
        })
    })
}

struct GnomeShellVideoBackend {
    stop_tx: mpsc::Sender<()>,
    worker: std::thread::JoinHandle<anyhow::Result<()>>,
    output_path: PathBuf,
    started_at: Instant,
}

impl GnomeShellVideoBackend {
    fn start(output_path: &Path, first_frame: Vec<u8>) -> anyhow::Result<Self> {
        let ffmpeg = require_ffmpeg("GNOME Wayland")?;
        prepare_output_dir(output_path)?;

        let output_path = output_path.to_path_buf();
        let worker_output = output_path.clone();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result =
                run_gnome_shell_encoder(&ffmpeg, &worker_output, first_frame, stop_rx, &ready_tx);
            if result.is_err() {
                let _ = ready_tx.send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
            }
            result
        });
        match ready_rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(())) => Ok(Self {
                stop_tx,
                worker,
                output_path,
                started_at: Instant::now(),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                anyhow::bail!("GNOME Wayland video failed to start: {error}")
            }
            Err(error) => {
                let _ = stop_tx.send(());
                let _ = worker.join();
                anyhow::bail!("GNOME Wayland video startup timed out: {error}")
            }
        }
    }
}

fn run_gnome_shell_encoder(
    ffmpeg: &Path,
    output_path: &Path,
    first_frame: Vec<u8>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: &mpsc::SyncSender<Result<(), String>>,
) -> anyhow::Result<()> {
    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "image2pipe",
            "-framerate",
            "5",
            "-vcodec",
            "png",
            "-i",
            "-",
            "-an",
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-y",
        ])
        .arg(output_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow::anyhow!("failed to start GNOME FFmpeg encoder: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("GNOME FFmpeg encoder exposed no stdin"))?;
    stdin.write_all(&first_frame)?;
    stdin.flush()?;
    let _ = ready_tx.send(Ok(()));

    loop {
        match stop_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let frame = crate::wayland::shell_helper::trusted_screenshot_display()
                    .ok_or_else(|| anyhow::anyhow!("trusted GNOME compositor capture stopped"))?;
                stdin.write_all(&frame)?;
                stdin.flush()?;
            }
        }
    }
    drop(stdin);
    let output = child.wait_with_output()?;
    if !output.status.success() {
        anyhow::bail!(
            "GNOME FFmpeg encoder exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if output_path.metadata().map(|meta| meta.len()).unwrap_or(0) == 0 {
        anyhow::bail!("GNOME FFmpeg encoder produced an empty artifact");
    }
    Ok(())
}

impl VideoBackend for GnomeShellVideoBackend {
    fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();
        let _ = self.stop_tx.send(());
        match self.worker.join() {
            Ok(result) => result?,
            Err(_) => anyhow::bail!("GNOME Wayland video worker panicked"),
        }
        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized: true,
        })
    }
}

struct WfRecorderVideoBackend {
    child: Child,
    output_path: PathBuf,
    started_at: Instant,
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl WfRecorderVideoBackend {
    fn start(output_path: &Path) -> anyhow::Result<Self> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            anyhow::bail!("wf-recorder requires WAYLAND_DISPLAY");
        }
        let available = Command::new("wf-recorder")
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !available {
            anyhow::bail!(
                "wf-recorder not found on PATH. Install wf-recorder for native Wayland video."
            );
        }
        prepare_output_dir(output_path)?;

        let started_at = Instant::now();
        let mut command = Command::new("wf-recorder");
        command
            .arg("-f")
            .arg(output_path)
            .args(["--no-damage", "-c", "libx264", "-x", "yuv420p"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(output) = std::env::var_os("CUA_WAYLAND_RECORDING_OUTPUT") {
            command.arg("-o").arg(output);
        }
        let mut child = command
            .spawn()
            .map_err(|error| anyhow::anyhow!("failed to start wf-recorder: {error}"))?;
        let stderr_thread = drain_stderr_tail(&mut child);

        let probe_deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < probe_deadline {
            if let Some(status) = child.try_wait()? {
                let stderr = stderr_thread
                    .map(|worker| worker.join().unwrap_or_default())
                    .unwrap_or_default();
                anyhow::bail!(
                    "wf-recorder exited during startup ({status}): {}",
                    String::from_utf8_lossy(&stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Ok(Self {
            child,
            output_path: output_path.to_path_buf(),
            started_at,
            stderr_thread,
        })
    }
}

impl VideoBackend for WfRecorderVideoBackend {
    fn stop(mut self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGINT);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let finalized = loop {
            if let Some(status) = self.child.try_wait()? {
                break status.success();
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                break false;
            }
            std::thread::sleep(Duration::from_millis(80));
        };
        let stderr = self
            .stderr_thread
            .take()
            .and_then(|worker| worker.join().ok())
            .unwrap_or_default();
        let has_video = self
            .output_path
            .metadata()
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false);
        if !finalized || !has_video {
            anyhow::bail!(
                "wf-recorder did not finalize a playable artifact: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized: true,
        })
    }
}

/// Constant-frame-rate pacing for a producer that only delivers frames on
/// damage. Each tick reports how many copies of the latest frame bring the
/// encoded timeline up to wall-clock time, so a static desktop still yields a
/// video whose duration matches the recording.
#[derive(Debug)]
#[cfg_attr(not(feature = "portal-capture"), allow(dead_code))]
struct FramePacer {
    fps: u32,
    started_at: Instant,
    emitted: u64,
}

#[cfg_attr(not(feature = "portal-capture"), allow(dead_code))]
impl FramePacer {
    fn new(fps: u32, started_at: Instant) -> Self {
        Self {
            fps,
            started_at,
            emitted: 0,
        }
    }

    fn due(&mut self, now: Instant) -> u64 {
        let elapsed = now.saturating_duration_since(self.started_at);
        let target = elapsed.as_micros() * u128::from(self.fps) / 1_000_000;
        let due = u64::try_from(target)
            .unwrap_or(u64::MAX)
            .saturating_sub(self.emitted);
        self.emitted += due;
        due
    }
}

#[cfg(feature = "portal-capture")]
mod portal_video {
    use super::*;
    use crate::wayland::portal::RestoreToken;
    use crate::wayland::portal_screencast::{
        run_frame_stream, Frame, FrameFlow, FrameSink, ScreencastSession, StreamOptions,
        CONSENT_TIMEOUT,
    };
    use ashpd::desktop::screencast::SourceType;
    use ashpd::enumflags2::BitFlags;
    use libspa::param::video::VideoFormat;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    pub(super) const FPS: u32 = 10;
    const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
    /// Encoder must survive this long after its first frame before the
    /// backend reports a successful start; ffmpeg rejects bad arguments or
    /// missing codecs within milliseconds.
    const ENCODER_PROBE: Duration = Duration::from_millis(1500);
    const ENCODER_SHUTDOWN: Duration = Duration::from_secs(10);

    pub(super) struct PortalVideoBackend {
        stop: Arc<AtomicBool>,
        worker: std::thread::JoinHandle<anyhow::Result<()>>,
        output_path: PathBuf,
        started_at: Instant,
    }

    impl PortalVideoBackend {
        pub(super) fn start(output_path: &Path) -> anyhow::Result<Self> {
            let ffmpeg = require_ffmpeg("portal ScreenCast")?;
            prepare_output_dir(output_path)?;

            let output_path = output_path.to_path_buf();
            let stop = Arc::new(AtomicBool::new(false));
            // Success carries the instant the encoded timeline began, so the
            // reported duration matches the frames on disk rather than the
            // consent wait that preceded them.
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<Instant, String>>(1);
            let worker = std::thread::spawn({
                let output_path = output_path.clone();
                let stop = stop.clone();
                move || {
                    let session = ScreencastSession::open(
                        BitFlags::from(SourceType::Monitor),
                        Some(RestoreToken::ScreencastVideo),
                        CONSENT_TIMEOUT,
                    );
                    let session = match session {
                        Ok(session) => session,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return Err(error);
                        }
                    };
                    let result = (|| {
                        let fd = session.pipewire_fd()?;
                        let node_id = session.node_id()?;
                        let sink =
                            RawVideoEncoder::new(ffmpeg, output_path, stop, ready_tx.clone());
                        let options = StreamOptions {
                            first_frame_timeout: FIRST_FRAME_TIMEOUT,
                            tick_interval: Duration::from_secs(1) / FPS,
                        };
                        run_frame_stream(fd, node_id, options, sink)?.finish()
                    })();
                    session.close();
                    if let Err(error) = &result {
                        let _ = ready_tx.try_send(Err(error.to_string()));
                    }
                    result
                }
            });
            let start_deadline = CONSENT_TIMEOUT + FIRST_FRAME_TIMEOUT + ENCODER_PROBE * 2;
            match ready_rx.recv_timeout(start_deadline) {
                Ok(Ok(started_at)) => Ok(Self {
                    stop,
                    worker,
                    output_path,
                    started_at,
                }),
                Ok(Err(error)) => {
                    let _ = worker.join();
                    anyhow::bail!("portal ScreenCast video failed to start: {error}")
                }
                Err(_) => {
                    stop.store(true, Ordering::SeqCst);
                    let _ = worker.join();
                    anyhow::bail!(
                        "portal ScreenCast video did not start within {}s",
                        start_deadline.as_secs()
                    )
                }
            }
        }
    }

    impl VideoBackend for PortalVideoBackend {
        fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
            let elapsed = self.started_at.elapsed();
            self.stop.store(true, Ordering::SeqCst);
            match self.worker.join() {
                Ok(result) => result?,
                Err(_) => anyhow::bail!("portal ScreenCast video worker panicked"),
            }
            Ok(VideoMetadata {
                path: self.output_path,
                duration_ms: elapsed.as_millis() as u64,
                finalized: true,
            })
        }
    }

    /// ffmpeg `rawvideo` pixel format matching a negotiated PipeWire format.
    pub(super) fn ffmpeg_pix_fmt(format: VideoFormat) -> Option<&'static str> {
        match format {
            VideoFormat::BGRx => Some("bgr0"),
            VideoFormat::BGRA => Some("bgra"),
            VideoFormat::RGBx => Some("rgb0"),
            VideoFormat::RGBA => Some("rgba"),
            _ => None,
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Geometry {
        width: u32,
        height: u32,
        pix_fmt: &'static str,
    }

    struct Encoder {
        child: Child,
        stdin: Option<std::process::ChildStdin>,
        stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
        geometry: Geometry,
        spawned_at: Instant,
        pacer: FramePacer,
    }

    impl Encoder {
        fn spawn(ffmpeg: &Path, output_path: &Path, geometry: Geometry) -> anyhow::Result<Self> {
            let mut child = Command::new(ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
                .args(["-pix_fmt", geometry.pix_fmt])
                .args([
                    "-video_size",
                    &format!("{}x{}", geometry.width, geometry.height),
                ])
                .args(["-framerate", &FPS.to_string()])
                .args(["-i", "-", "-an"])
                .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"])
                .args(["-c:v", "libx264", "-preset", "ultrafast"])
                .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart", "-y"])
                .arg(output_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| anyhow::anyhow!("failed to start FFmpeg encoder: {error}"))?;
            let stdin = child.stdin.take();
            let stderr_thread = drain_stderr_tail(&mut child);
            let now = Instant::now();
            Ok(Self {
                child,
                stdin,
                stderr_thread,
                geometry,
                spawned_at: now,
                pacer: FramePacer::new(FPS, now),
            })
        }

        fn stderr_tail(&mut self) -> String {
            let tail = self
                .stderr_thread
                .take()
                .and_then(|worker| worker.join().ok())
                .unwrap_or_default();
            String::from_utf8_lossy(&tail).into_owned()
        }

        fn ensure_alive(&mut self) -> anyhow::Result<()> {
            if let Some(status) = self.child.try_wait()? {
                let tail = self.stderr_tail();
                anyhow::bail!("FFmpeg encoder exited during recording ({status}): {tail}");
            }
            Ok(())
        }

        /// Close stdin so ffmpeg writes the moov atom, then wait for it.
        fn finish(mut self) -> anyhow::Result<()> {
            drop(self.stdin.take());
            let deadline = Instant::now() + ENCODER_SHUTDOWN;
            let status = loop {
                if let Some(status) = self.child.try_wait()? {
                    break Some(status);
                }
                if Instant::now() >= deadline {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(80));
            };
            let tail = self.stderr_tail();
            match status {
                Some(status) if status.success() => Ok(()),
                Some(status) => anyhow::bail!("FFmpeg encoder exited with {status}: {tail}"),
                None => anyhow::bail!(
                    "FFmpeg encoder did not finalize within {}s: {tail}",
                    ENCODER_SHUTDOWN.as_secs()
                ),
            }
        }
    }

    impl Drop for Encoder {
        fn drop(&mut self) {
            // Reached only on error paths; the success path consumes the
            // encoder through `finish`. Closing stdin lets ffmpeg finalize a
            // playable partial artifact before we reap it.
            drop(self.stdin.take());
            let deadline = Instant::now() + ENCODER_SHUTDOWN;
            while self.child.try_wait().ok().flatten().is_none() {
                if Instant::now() >= deadline {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(80));
            }
        }
    }

    /// Feeds paced raw frames from the portal stream into an FFmpeg encoder
    /// that is spawned lazily with the first frame's negotiated geometry.
    struct RawVideoEncoder {
        ffmpeg: PathBuf,
        output_path: PathBuf,
        stop: Arc<AtomicBool>,
        ready: Option<mpsc::SyncSender<Result<Instant, String>>>,
        encoder: Option<Encoder>,
        /// Latest frame, tightly packed at `width * 4` bytes per row.
        latest: Vec<u8>,
    }

    impl RawVideoEncoder {
        fn new(
            ffmpeg: PathBuf,
            output_path: PathBuf,
            stop: Arc<AtomicBool>,
            ready: mpsc::SyncSender<Result<Instant, String>>,
        ) -> Self {
            Self {
                ffmpeg,
                output_path,
                stop,
                ready: Some(ready),
                encoder: None,
                latest: Vec::new(),
            }
        }

        fn finish(mut self) -> anyhow::Result<()> {
            let Some(encoder) = self.encoder.take() else {
                anyhow::bail!("portal ScreenCast delivered no frame before recording stopped");
            };
            encoder.finish()?;
            if self
                .output_path
                .metadata()
                .map(|meta| meta.len())
                .unwrap_or(0)
                == 0
            {
                anyhow::bail!("FFmpeg encoder produced an empty artifact");
            }
            Ok(())
        }
    }

    impl FrameSink for RawVideoEncoder {
        fn frame(&mut self, frame: Frame<'_>) -> anyhow::Result<FrameFlow> {
            let pix_fmt = ffmpeg_pix_fmt(frame.format).ok_or_else(|| {
                anyhow::anyhow!(
                    "portal ScreenCast negotiated unsupported pixel format {:?}",
                    frame.format
                )
            })?;
            let geometry = Geometry {
                width: frame.width,
                height: frame.height,
                pix_fmt,
            };
            match &self.encoder {
                None => {
                    self.encoder = Some(Encoder::spawn(&self.ffmpeg, &self.output_path, geometry)?);
                }
                Some(encoder) if encoder.geometry != geometry => {
                    anyhow::bail!(
                        "display geometry changed from {}x{} {} to {}x{} {} during recording; \
                         the artifact was finalized at the point of change",
                        encoder.geometry.width,
                        encoder.geometry.height,
                        encoder.geometry.pix_fmt,
                        geometry.width,
                        geometry.height,
                        geometry.pix_fmt
                    );
                }
                Some(_) => {}
            }
            let row = frame.width as usize * 4;
            self.latest.clear();
            if frame.stride as usize == row {
                self.latest.extend_from_slice(frame.pixels);
            } else {
                for chunk in frame.pixels.chunks_exact(frame.stride as usize) {
                    self.latest.extend_from_slice(&chunk[..row]);
                }
            }
            Ok(FrameFlow::Continue)
        }

        fn tick(&mut self) -> anyhow::Result<FrameFlow> {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(FrameFlow::Stop);
            }
            let Some(encoder) = self.encoder.as_mut() else {
                return Ok(FrameFlow::Continue);
            };
            encoder.ensure_alive()?;
            let due = encoder.pacer.due(Instant::now());
            if due > 0 {
                let stdin = encoder
                    .stdin
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("FFmpeg encoder exposed no stdin"))?;
                for _ in 0..due {
                    stdin.write_all(&self.latest).map_err(|error| {
                        anyhow::anyhow!("failed to feed FFmpeg encoder: {error}")
                    })?;
                }
                stdin.flush()?;
            }
            if self.ready.is_some()
                && encoder.pacer.emitted > 0
                && encoder.spawned_at.elapsed() >= ENCODER_PROBE
            {
                encoder.ensure_alive()?;
                if let Some(ready) = self.ready.take() {
                    let _ = ready.send(Ok(encoder.pacer.started_at));
                }
            }
            Ok(FrameFlow::Continue)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_gnome_frames_take_precedence_over_every_probe_result() {
        for probe in [Some(true), Some(false), None] {
            assert_eq!(select_route(true, probe), WaylandVideoRoute::GnomeShell);
        }
    }

    #[test]
    fn compositors_without_wlr_screencopy_route_to_the_portal() {
        assert_eq!(
            select_route(false, Some(false)),
            WaylandVideoRoute::PortalScreencast
        );
    }

    #[test]
    fn wlr_screencopy_and_unprobeable_registries_keep_wf_recorder() {
        assert_eq!(
            select_route(false, Some(true)),
            WaylandVideoRoute::WfRecorder
        );
        assert_eq!(select_route(false, None), WaylandVideoRoute::WfRecorder);
    }

    #[cfg(not(feature = "portal-capture"))]
    #[test]
    fn portal_route_without_the_feature_names_the_build_flag() {
        let error = start_portal_video(Path::new("/nonexistent/recording.mp4"))
            .err()
            .expect("portal video must be refused without portal-capture");
        let message = error.to_string();
        assert!(message.contains("portal-capture"), "{message}");
        assert!(message.contains("wlr-screencopy"), "{message}");
        assert!(
            !Path::new("/nonexistent").exists(),
            "refusal must not create output directories"
        );
    }

    #[test]
    fn pacer_repeats_the_latest_frame_to_match_wall_clock_time() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(10, start);
        assert_eq!(pacer.due(start), 0);
        assert_eq!(pacer.due(start + Duration::from_millis(50)), 0);
        assert_eq!(pacer.due(start + Duration::from_millis(100)), 1);
        assert_eq!(pacer.due(start + Duration::from_millis(150)), 0);
        // A stall of one second owes ten frames, not one.
        assert_eq!(pacer.due(start + Duration::from_millis(1150)), 10);
        assert_eq!(pacer.emitted, 11);
    }

    #[test]
    fn pacer_never_owes_frames_for_time_before_it_started() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(30, start + Duration::from_secs(5));
        assert_eq!(pacer.due(start), 0);
        assert_eq!(pacer.due(start + Duration::from_secs(6)), 30);
    }

    #[cfg(feature = "portal-capture")]
    #[test]
    fn negotiated_formats_map_to_rawvideo_pixel_formats() {
        use libspa::param::video::VideoFormat;
        assert_eq!(
            portal_video::ffmpeg_pix_fmt(VideoFormat::BGRx),
            Some("bgr0")
        );
        assert_eq!(
            portal_video::ffmpeg_pix_fmt(VideoFormat::BGRA),
            Some("bgra")
        );
        assert_eq!(
            portal_video::ffmpeg_pix_fmt(VideoFormat::RGBx),
            Some("rgb0")
        );
        assert_eq!(
            portal_video::ffmpeg_pix_fmt(VideoFormat::RGBA),
            Some("rgba")
        );
        assert_eq!(portal_video::ffmpeg_pix_fmt(VideoFormat::NV12), None);
    }
}
