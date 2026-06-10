//! Linux video-recording backend.
//!
//! On Wayland (Hyprland) the stock ffmpeg `x11grab` input only sees the
//! XWayland root — native Wayland surfaces never appear, so screen
//! recordings come out black or XWayland-only. This backend captures real
//! compositor output frames via wlr-screencopy
//! (`crate::wayland_capture::ScreencopyCapturer`) and pipes them to an
//! ffmpeg subprocess as rawvideo on stdin (libx264, same encode settings
//! as the core ffmpeg backend).
//!
//! Frame pacing: the encoder is fed at a fixed nominal FPS against the
//! wall clock. When a capture iteration runs slow, the last good frame is
//! duplicated to catch up, so the mp4's duration tracks wall time and the
//! per-turn `t_ms_from_session_start` timestamps in action.json line up
//! with video time (the render zoom pipeline depends on this).
//!
//! `LinuxVideoBackendFactory` picks per session: Wayland screencopy when
//! `WAYLAND_DISPLAY` is set (falling back to x11grab if the Wayland path
//! fails to start), else the core ffmpeg x11grab backend.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cua_driver_core::video::{VideoBackend, VideoBackendFactory, VideoMetadata};

use crate::wayland_capture::{ffmpeg_pixel_format, FrameInfo, ScreencopyCapturer};

const FPS: u32 = 30;
/// Give up on the capture thread after this many consecutive failed frames:
/// the compositor is gone or the output vanished. Each failed attempt can
/// burn up to the 4 s dispatch deadline, so worst case this is ~40 s of
/// retrying, not a frame-count worth of wall time.
const MAX_CONSECUTIVE_FAILURES: u32 = 10;

pub struct LinuxVideoBackendFactory;

impl VideoBackendFactory for LinuxVideoBackendFactory {
    fn start(&self, output_path: &Path) -> Result<Box<dyn VideoBackend>> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            // No x11grab fallback on Wayland sessions: it records only the
            // XWayland root (black / partial), which would silently
            // masquerade as a healthy recording. Surfacing the error puts
            // it in session.json video.error where the user can see it.
            return WaylandVideoBackend::start(output_path)
                .map(|b| Box::new(b) as Box<dyn VideoBackend>)
                .map_err(|e| e.context("Wayland screencopy video failed"));
        }
        cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory.start(output_path)
    }
}

pub struct WaylandVideoBackend {
    child: Child,
    output_path: PathBuf,
    started_at: Instant,
    stop: Arc<AtomicBool>,
    capture_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl WaylandVideoBackend {
    fn start(output_path: &Path) -> Result<Self> {
        let output_path = output_path.to_path_buf();

        // Open the capture session and grab one frame up front: it proves
        // the protocol path works and pins the frame geometry/format for
        // the ffmpeg invocation.
        let focused = crate::hyprland::focused_monitor_name();
        let mut capturer = ScreencopyCapturer::open(focused.as_deref())
            .context("wlr-screencopy session open failed")?;
        let mut first_frame = Vec::new();
        let info = capturer
            .capture_frame(true, &mut first_frame)
            .context("initial screencopy frame failed")?;
        let pix_fmt = ffmpeg_pixel_format(info.format)
            .with_context(|| format!("unsupported screencopy pixel format {:?}", info.format))?;

        let ffmpeg = cua_driver_core::video_ffmpeg::find_ffmpeg().context(
            "ffmpeg not found on PATH. Install with: apt install ffmpeg (Debian/Ubuntu) \
             or pacman -S ffmpeg (Arch).",
        )?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!(
                    "failed to create recording output directory {}: {e}",
                    parent.display()
                )
            })?;
        }

        let mut cmd = Command::new(&ffmpeg);
        cmd.arg("-y")
            .arg("-loglevel").arg("error")
            .arg("-f").arg("rawvideo")
            .arg("-pixel_format").arg(pix_fmt)
            .arg("-video_size").arg(format!("{}x{}", info.width, info.height))
            .arg("-framerate").arg(FPS.to_string())
            .arg("-i").arg("pipe:0")
            // yuv420p needs even dimensions; pad rather than crop (matches
            // the core ffmpeg backend).
            .arg("-vf").arg("pad=ceil(iw/2)*2:ceil(ih/2)*2")
            .arg("-c:v").arg("libx264")
            .arg("-preset").arg("ultrafast")
            .arg("-pix_fmt").arg("yuv420p")
            .arg("-movflags").arg("+faststart")
            .arg("-g").arg("30")
            .arg(&output_path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn ffmpeg ({}): {e}", ffmpeg.display()))?;

        let stderr_thread = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || -> Vec<u8> {
                use std::io::Read;
                let mut buf = Vec::with_capacity(4096);
                let _ = stderr.read_to_end(&mut buf);
                let len = buf.len();
                if len > 4096 {
                    buf.drain(..len - 4096);
                }
                buf
            })
        });

        let mut stdin = child.stdin.take().context("ffmpeg stdin unavailable")?;
        let stop = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();

        let capture_thread = {
            let stop = Arc::clone(&stop);
            let spawned = std::thread::Builder::new()
                .name("wl-video-capture".into())
                .spawn(move || {
                    capture_loop(capturer, info, first_frame, &mut stdin, &stop, started_at);
                    // Dropping stdin sends EOF — ffmpeg finalizes the mp4.
                    drop(stdin);
                });
            match spawned {
                Ok(handle) => handle,
                Err(e) => {
                    // Without the feeder thread ffmpeg would linger as a
                    // zombie blocked on an empty pipe.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e).context("failed to spawn capture thread");
                }
            }
        };

        Ok(WaylandVideoBackend {
            child,
            output_path,
            started_at,
            stop,
            capture_thread: Some(capture_thread),
            stderr_thread: Some(stderr_thread).flatten(),
        })
    }
}

/// Feed ffmpeg at a fixed nominal FPS against the wall clock, duplicating
/// the last good frame when capture runs slow and tolerating transient
/// capture failures.
fn capture_loop(
    mut capturer: ScreencopyCapturer,
    first_info: FrameInfo,
    mut latest: Vec<u8>,
    stdin: &mut std::process::ChildStdin,
    stop: &AtomicBool,
    started_at: Instant,
) {
    let mut scratch: Vec<u8> = Vec::with_capacity(latest.len());
    let mut frames_written: u64 = 0;
    let mut consecutive_failures: u32 = 0;
    let frame_interval = 1.0 / FPS as f64;

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        // Keep the encoder fed up to the wall clock (duplicates catch up
        // after slow captures). Bounded per iteration: when the encoder
        // back-pressures (write_all blocks), an unbounded catch-up loop
        // would keep accumulating debt and never observe the stop flag.
        // Dropping backlog trades a brief slow-motion segment for bounded
        // latency; FPS duplicates per pass caps a single iteration's
        // writes at one second of video.
        let due = (started_at.elapsed().as_secs_f64() / frame_interval) as u64 + 1;
        if due - frames_written > FPS as u64 * 2 {
            tracing::warn!(target: "recording",
                "video encoder back-pressure: dropping {} frames of backlog",
                due - frames_written - 1);
            frames_written = due - 1;
        }
        let mut wrote_this_pass = 0u32;
        while frames_written < due && wrote_this_pass < FPS {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if stdin.write_all(&latest).is_err() {
                // ffmpeg died; nothing more to do.
                return;
            }
            frames_written += 1;
            wrote_this_pass += 1;
        }

        match capturer.capture_frame(true, &mut scratch) {
            Ok(info) if info == first_info => {
                std::mem::swap(&mut latest, &mut scratch);
                consecutive_failures = 0;
            }
            Ok(info) => {
                tracing::warn!(target: "recording",
                    "screencopy frame geometry changed ({}x{} -> {}x{}); freezing video frame",
                    first_info.width, first_info.height, info.width, info.height);
                // Output mode/scale changed mid-recording: the rawvideo
                // geometry is fixed, so keep duplicating the last good
                // frame rather than corrupting the stream.
                consecutive_failures = 0;
            }
            Err(e) => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    tracing::warn!(target: "recording",
                        "screencopy capture failing persistently ({e:#}); stopping video frames");
                    return;
                }
            }
        }

        // Sleep to the next frame boundary.
        let next = started_at + Duration::from_secs_f64(frames_written as f64 * frame_interval);
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
}

impl VideoBackend for WaylandVideoBackend {
    fn stop(mut self: Box<Self>) -> Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();

        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.capture_thread.take() {
            // Bounded wait: the thread can be stuck in write_all against a
            // wedged encoder, and stop() runs under the daemon-wide
            // recording mutex — an unbounded join here would hang every
            // recorded tool call. Killing ffmpeg breaks the pipe (EPIPE),
            // which unblocks write_all and lets the thread exit.
            let deadline = Instant::now() + Duration::from_millis(3000);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if !handle.is_finished() {
                tracing::warn!(target: "recording",
                    "video capture thread stuck (encoder back-pressure?); killing ffmpeg");
                let _ = self.child.kill();
            }
            let _ = handle.join(); // drops ffmpeg stdin -> EOF
        }

        // Encoder flush on EOF; 4K ultrafast flush fits comfortably in 5 s.
        let deadline = Instant::now() + Duration::from_millis(5000);
        let finalized;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    finalized = status.success();
                    break;
                }
                Ok(None) => {
                    if Instant::now() > deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        finalized = false;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
                Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    finalized = false;
                    break;
                }
            }
        }

        if let Some(handle) = self.stderr_thread.take() {
            if let Ok(buf) = handle.join() {
                if !finalized && !buf.is_empty() {
                    let tail = String::from_utf8_lossy(&buf);
                    tracing::warn!(target: "recording",
                        "ffmpeg did not finalize cleanly. Last stderr tail:\n{tail}");
                }
            }
        }

        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized,
        })
    }
}
