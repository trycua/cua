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
         unavailable. Use a build with `--features portal-capture` (the released Linux \
         binaries and the Nix package enable it) to record on KDE/KWin and other portal-only \
         compositors."
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
        run_frame_stream, Frame, FrameFlow, FrameSink, RawFormat, ScreencastSession, StreamOptions,
        CONSENT_TIMEOUT,
    };
    use ashpd::desktop::screencast::SourceType;
    use ashpd::enumflags2::BitFlags;
    use std::io::ErrorKind;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    pub(super) const FPS: u32 = 10;
    const TICK: Duration = Duration::from_millis(1000 / FPS as u64);
    const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
    /// Encoder must survive this long after its first frame before the
    /// backend reports a successful start; ffmpeg rejects bad arguments or
    /// missing codecs within milliseconds.
    const ENCODER_PROBE: Duration = Duration::from_millis(1500);
    const ENCODER_SHUTDOWN: Duration = Duration::from_secs(10);
    /// An encoder that accepts no frame bytes for this long while frames are
    /// owed is treated as wedged rather than slow.
    pub(super) const ENCODER_STALL: Duration = Duration::from_secs(5);
    /// Upper bound on the time one tick spends pushing bytes into the
    /// encoder pipe, so the PipeWire loop stays responsive to stop requests.
    const WRITE_BUDGET: Duration = Duration::from_millis(1000 / FPS as u64 / 2);

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
                            tick_interval: TICK,
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

    /// ffmpeg `rawvideo` pixel format matching a negotiated PipeWire layout.
    pub(super) fn ffmpeg_pix_fmt(format: RawFormat) -> &'static str {
        match format {
            RawFormat::BGRx => "bgr0",
            RawFormat::BGRA => "bgra",
            RawFormat::RGBx => "rgb0",
            RawFormat::RGBA => "rgba",
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Geometry {
        width: u32,
        height: u32,
        pix_fmt: &'static str,
    }

    pub(super) fn set_nonblocking(fd: &impl AsRawFd) -> std::io::Result<()> {
        let fd = fd.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Frames owed to the encoder that have not been fully written yet.
    ///
    /// Writes go to a non-blocking pipe and resume where they left off, so a
    /// consumer that stops reading can never park the PipeWire loop; it shows
    /// up as a stall instead.
    pub(super) struct Outbox {
        /// Snapshot of the frame currently being written, so a newer capture
        /// arriving mid-write cannot tear it.
        frame: Vec<u8>,
        written: usize,
        owed: u64,
        pub(super) delivered: u64,
        last_progress: Instant,
    }

    impl Outbox {
        pub(super) fn new(now: Instant) -> Self {
            Self {
                frame: Vec::new(),
                written: 0,
                owed: 0,
                delivered: 0,
                last_progress: now,
            }
        }

        pub(super) fn owe(&mut self, frames: u64, now: Instant) {
            if frames == 0 {
                return;
            }
            if self.owed == 0 {
                self.last_progress = now;
            }
            self.owed += frames;
        }

        /// Push owed copies of `latest` into `dst` until it would block, the
        /// backlog is drained, or `budget` has elapsed since `now`.
        pub(super) fn deliver(
            &mut self,
            dst: &mut impl Write,
            latest: &[u8],
            now: Instant,
            budget: Duration,
        ) -> std::io::Result<()> {
            let deadline = now + budget;
            while self.owed > 0 {
                if self.written == 0 {
                    self.frame.clear();
                    self.frame.extend_from_slice(latest);
                }
                match dst.write(&self.frame[self.written..]) {
                    Ok(0) => return Err(ErrorKind::WriteZero.into()),
                    Ok(n) => {
                        self.written += n;
                        self.last_progress = now;
                        if self.written == self.frame.len() {
                            self.written = 0;
                            self.owed -= 1;
                            self.delivered += 1;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
            Ok(())
        }

        /// Frames are owed but none of their bytes were accepted for `limit`.
        pub(super) fn stalled(&self, now: Instant, limit: Duration) -> bool {
            self.owed > 0 && now.saturating_duration_since(self.last_progress) >= limit
        }

        pub(super) fn backlog(&self) -> u64 {
            self.owed
        }
    }

    struct Encoder {
        child: Child,
        stdin: Option<std::process::ChildStdin>,
        stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
        geometry: Geometry,
        spawned_at: Instant,
        pacer: FramePacer,
        outbox: Outbox,
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
            if let Some(stdin) = &stdin {
                if let Err(error) = set_nonblocking(stdin) {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("could not make the FFmpeg encoder pipe non-blocking: {error}");
                }
            }
            let stderr_thread = drain_stderr_tail(&mut child);
            let now = Instant::now();
            Ok(Self {
                child,
                stdin,
                stderr_thread,
                geometry,
                spawned_at: now,
                pacer: FramePacer::new(FPS, now),
                outbox: Outbox::new(now),
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
        /// Frames still owed at this point are dropped, so an encoder that
        /// fell behind real time yields a proportionally shorter video.
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
            let geometry = Geometry {
                width: frame.width(),
                height: frame.height(),
                pix_fmt: ffmpeg_pix_fmt(frame.format()),
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
            frame.pack_rows(&mut self.latest);
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
            let now = Instant::now();
            let due = encoder.pacer.due(now);
            encoder.outbox.owe(due, now);
            let stdin = encoder
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("FFmpeg encoder exposed no stdin"))?;
            encoder
                .outbox
                .deliver(stdin, &self.latest, now, WRITE_BUDGET)
                .map_err(|error| anyhow::anyhow!("failed to feed FFmpeg encoder: {error}"))?;
            if encoder.outbox.stalled(now, ENCODER_STALL) {
                anyhow::bail!(
                    "FFmpeg encoder accepted no frame data for {}s with {} frames waiting",
                    ENCODER_STALL.as_secs(),
                    encoder.outbox.backlog()
                );
            }
            if self.ready.is_some()
                && encoder.outbox.delivered > 0
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
        use crate::wayland::portal_screencast::RawFormat;
        assert_eq!(portal_video::ffmpeg_pix_fmt(RawFormat::BGRx), "bgr0");
        assert_eq!(portal_video::ffmpeg_pix_fmt(RawFormat::BGRA), "bgra");
        assert_eq!(portal_video::ffmpeg_pix_fmt(RawFormat::RGBx), "rgb0");
        assert_eq!(portal_video::ffmpeg_pix_fmt(RawFormat::RGBA), "rgba");
    }

    /// A non-blocking pipe whose reader never drains it unless asked: the
    /// shape of a wedged encoder.
    #[cfg(feature = "portal-capture")]
    fn stuck_pipe() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd;
        let mut fds = [0i32; 2];
        assert_eq!(
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
            0,
            "pipe2: {}",
            std::io::Error::last_os_error()
        );
        unsafe {
            (
                std::fs::File::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
            )
        }
    }

    #[cfg(feature = "portal-capture")]
    #[test]
    fn outbox_never_blocks_on_a_consumer_that_stopped_reading() {
        use portal_video::{Outbox, ENCODER_STALL};
        // Larger than any Linux pipe capacity the kernel hands out by default.
        let frame = vec![0xABu8; 2 << 20];
        let (_reader, mut writer) = stuck_pipe();
        let now = Instant::now();
        let mut outbox = Outbox::new(now);
        outbox.owe(3, now);

        let began = Instant::now();
        outbox
            .deliver(&mut writer, &frame, now, Duration::from_millis(50))
            .expect("a full pipe is back-pressure, not an error");
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "delivery must return once the pipe is full"
        );
        assert_eq!(outbox.delivered, 0);
        assert_eq!(outbox.backlog(), 3);
        assert!(!outbox.stalled(now, ENCODER_STALL));
        assert!(outbox.stalled(now + ENCODER_STALL, ENCODER_STALL));
    }

    #[cfg(feature = "portal-capture")]
    #[test]
    fn outbox_resumes_partial_frames_and_counts_only_complete_ones() {
        use portal_video::{Outbox, ENCODER_STALL};
        let frame: Vec<u8> = (0..(2u32 << 20)).map(|i| (i % 251) as u8).collect();
        let (mut reader, mut writer) = stuck_pipe();
        let now = Instant::now();
        let mut outbox = Outbox::new(now);
        outbox.owe(2, now);

        let mut received = Vec::new();
        let mut scratch = vec![0u8; 1 << 16];
        let mut later = now;
        while outbox.backlog() > 0 {
            later += Duration::from_millis(100);
            outbox
                .deliver(&mut writer, &frame, later, Duration::from_millis(50))
                .unwrap();
            loop {
                match reader.read(&mut scratch) {
                    Ok(0) => break,
                    Ok(n) => received.extend_from_slice(&scratch[..n]),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => panic!("{error}"),
                }
            }
            assert!(
                !outbox.stalled(later, ENCODER_STALL),
                "a draining consumer keeps making progress"
            );
        }
        assert_eq!(outbox.delivered, 2);
        assert_eq!(received.len(), frame.len() * 2);
        assert_eq!(&received[..frame.len()], &frame[..]);
        assert_eq!(&received[frame.len()..], &frame[..]);
    }

    /// The encoder is a child process holding the read end of a pipe; a
    /// child that never reads must not park delivery on the PipeWire thread.
    #[cfg(feature = "portal-capture")]
    #[test]
    fn a_child_that_never_reads_its_stdin_cannot_park_delivery() {
        use portal_video::{set_nonblocking, Outbox};
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep is available on every Linux CI image");
        let mut stdin = child.stdin.take().unwrap();
        set_nonblocking(&stdin).unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let frame = vec![0xCDu8; 2 << 20];
            let now = Instant::now();
            let mut outbox = Outbox::new(now);
            outbox.owe(1, now);
            let result = outbox.deliver(&mut stdin, &frame, now, Duration::from_millis(50));
            let _ = done_tx.send(result.map(|()| outbox.backlog()));
        });
        let outcome = done_rx.recv_timeout(Duration::from_secs(3));
        let _ = child.kill();
        let _ = child.wait();
        match outcome {
            Ok(Ok(backlog)) => assert_eq!(backlog, 1, "the unread frame stays owed"),
            Ok(Err(error)) => panic!("back-pressure surfaced as an error: {error}"),
            Err(_) => panic!("delivery blocked on a child that never reads"),
        }
    }

    #[cfg(feature = "portal-capture")]
    #[test]
    fn outbox_measures_stalls_from_when_frames_became_owed() {
        use portal_video::{Outbox, ENCODER_STALL};
        let start = Instant::now();
        let mut outbox = Outbox::new(start);
        // Idle time before anything is owed must not count as a stall.
        let later = start + ENCODER_STALL * 3;
        outbox.owe(1, later);
        assert!(!outbox.stalled(later, ENCODER_STALL));
        assert!(!outbox.stalled(later + ENCODER_STALL / 2, ENCODER_STALL));
        assert!(outbox.stalled(later + ENCODER_STALL, ENCODER_STALL));
    }
}
