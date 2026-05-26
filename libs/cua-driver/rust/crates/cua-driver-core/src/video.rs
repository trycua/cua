//! Cross-platform screen-video capture via ffmpeg subprocess.
//!
//! Spawned by `RecordingSession` when video recording is requested. Each
//! platform uses ffmpeg with a platform-appropriate input device:
//!
//! - **Windows:** `gdigrab` (GDI screen-grab of the main display)
//! - **macOS:**   `avfoundation` (input "1" = main display, no audio)
//! - **Linux:**   `x11grab` (DISPLAY env var, defaults to `:0.0`)
//!
//! Encoder is `libx264 -preset ultrafast -pix_fmt yuv420p` everywhere —
//! produces a broadly playable MP4 at low CPU. Default framerate 30 fps.
//!
//! Lifecycle:
//!   1. `VideoRecorder::start(path)` spawns ffmpeg writing to `path`.
//!   2. Caller stays alive while recording.
//!   3. `recorder.stop()` sends ffmpeg `q` on stdin (clean shutdown that
//!      finalizes the mp4 moov atom) and waits for it to exit. On Windows
//!      `q\n` works the same way — ffmpeg's stdin handler reads any
//!      keypress and triggers `do_exit()`.
//!
//! When ffmpeg isn't on PATH or fails to start, `start()` returns a
//! structured error that the recording session surfaces to the MCP /
//! CLI caller.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One active video-capture process.
pub struct VideoRecorder {
    child: Child,
    output_path: PathBuf,
    started_at: Instant,
    /// Drains ffmpeg's stderr so its pipe doesn't fill up and block the
    /// encoder. The collected tail is read in `stop()` for diagnostics
    /// when ffmpeg exits non-zero.
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
}

/// Metadata returned by `VideoRecorder::stop()`; mirrors the Swift impl's
/// `FinalMetadata` so `session.json` carries the same shape across both
/// platforms.
#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub path: PathBuf,
    /// Wall-clock duration the recorder was active.
    pub duration_ms: u64,
    /// Whether ffmpeg exited cleanly (so the mp4 is playable).
    pub finalized: bool,
}

impl VideoRecorder {
    /// Spawn ffmpeg writing the main display to `output_path`.
    pub fn start(output_path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let output_path = output_path.into();

        // Resolve ffmpeg location. We probe `ffmpeg` (PATH lookup) and emit
        // a clear, actionable error when missing — ffmpeg is the one
        // runtime dep video recording carries, and a vague "command failed"
        // would just push debugging cost onto callers.
        let ffmpeg = match find_ffmpeg() {
            Some(p) => p,
            None => anyhow::bail!(
                "ffmpeg not found on PATH. Install with: \
                 winget install Gyan.FFmpeg (Windows), \
                 brew install ffmpeg (macOS), \
                 or apt install ffmpeg (Linux)."
            ),
        };

        // Make sure the parent directory exists.
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Platform-specific input device flags. Encoder + container flags
        // are shared.
        let mut cmd = Command::new(&ffmpeg);
        cmd.arg("-y") // overwrite existing output
            .arg("-loglevel").arg("error"); // we don't want ffmpeg's progress on stderr

        platform_input_args(&mut cmd);

        // yuv420p (the broad-compat pixel format we encode to) requires
        // BOTH width and height to be even. Most desktops aren't (taskbar
        // ate one row on this Win11 host: 1512×949). Pad by 1 px on the
        // bottom/right when needed so libx264 accepts the frame. Padding
        // beats cropping — keeps the full display in frame.
        cmd.arg("-vf").arg("pad=ceil(iw/2)*2:ceil(ih/2)*2");

        cmd.arg("-c:v").arg("libx264")
            .arg("-preset").arg("ultrafast")
            .arg("-pix_fmt").arg("yuv420p")
            .arg("-movflags").arg("+faststart")
            // Force a keyframe every 30 frames (1s @ 30fps) so a clean
            // stop has a recent IDR — keeps the final mp4 from being
            // unplayable if the encoder hadn't emitted a keyframe yet.
            .arg("-g").arg("30")
            .arg(&output_path);

        // We need stdin to send `q\n` for a clean shutdown that
        // finalizes the moov atom. Stderr is captured for diagnostics on
        // an unexpected exit; stdout is silenced (ffmpeg writes nothing
        // useful to stdout in this mode).
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!("Failed to spawn ffmpeg ({}): {e}", ffmpeg.display())
        })?;

        // Spawn a thread that drains stderr into a buffer. ffmpeg can be
        // verbose even with `-loglevel error` (filter setup warnings,
        // codec init notes), and a full stderr pipe blocks the encoder.
        // We keep the tail for diagnostics on a non-zero exit.
        let stderr_thread = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || -> Vec<u8> {
                use std::io::Read;
                let mut buf = Vec::with_capacity(4096);
                let _ = stderr.read_to_end(&mut buf);
                // Cap at last 4 KB so we don't unboundedly grow.
                let len = buf.len();
                if len > 4096 {
                    buf.drain(..len - 4096);
                }
                buf
            })
        });

        // Fast-fail probe. ffmpeg failures we care about:
        //  1. immediate exit (bad input device, missing codec) — surface stderr
        //  2. macOS: silent hang waiting on a TCC Screen Recording prompt that
        //     can't be displayed (subprocess of a daemon) — detected via
        //     no-frame-progress after 2 s.
        let probe_deadline = Instant::now() + Duration::from_millis(1500);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let tail = stderr_thread
                        .map(|h| h.join().unwrap_or_default())
                        .unwrap_or_default();
                    let tail_str = String::from_utf8_lossy(&tail);
                    let _ = child.kill();
                    anyhow::bail!(
                        "ffmpeg exited immediately ({status}). stderr tail:\n{tail_str}"
                    );
                }
                Ok(None) => {
                    if Instant::now() >= probe_deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => anyhow::bail!("ffmpeg try_wait failed: {e}"),
            }
        }

        // macOS-specific: ffmpeg + avfoundation Screen Recording permission
        // is per-binary, NOT inherited from the parent process. When a
        // daemon spawns ffmpeg the OS can't surface the consent dialog and
        // the subprocess blocks forever. Detect via no-output-file-grew
        // 2 s in, kill the child, return an actionable error.
        #[cfg(target_os = "macos")]
        {
            std::thread::sleep(Duration::from_millis(500));
            let progressed = std::fs::metadata(&output_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            if !progressed {
                let _ = child.kill();
                let _ = child.wait();
                let tail = stderr_thread
                    .map(|h| h.join().unwrap_or_default())
                    .unwrap_or_default();
                let tail_str = String::from_utf8_lossy(&tail);
                anyhow::bail!(
                    "ffmpeg appears to be blocked on the macOS Screen Recording TCC \
                     prompt. Open System Settings → Privacy & Security → Screen & \
                     System Audio Recording and grant access to your ffmpeg binary \
                     (e.g. /opt/homebrew/bin/ffmpeg), then restart cua-driver. Note: \
                     cua-driver itself having Screen Recording permission is NOT \
                     sufficient — the ffmpeg subprocess needs its own grant. A \
                     follow-up PR will replace ffmpeg with ScreenCaptureKit on macOS \
                     to remove this requirement. ffmpeg path: {ffmpeg_path}. stderr \
                     tail:\n{tail_str}",
                    ffmpeg_path = ffmpeg.display()
                );
            }
        }

        Ok(VideoRecorder {
            child,
            output_path,
            started_at: Instant::now(),
            stderr_thread,
        })
    }

    /// Gracefully terminate ffmpeg. Sends `q\n` on stdin (ffmpeg's clean
    /// shutdown trigger) and waits up to ~3 s for it to exit. Falls back
    /// to `kill()` if the polite path stalls — that leaves the mp4
    /// non-finalized (no moov atom), which we report as `finalized:
    /// false` so the caller can decide what to do with it.
    pub fn stop(mut self) -> anyhow::Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();
        let finalized;

        // Send the quit signal. If the stdin pipe is already dropped
        // (ffmpeg crashed early), proceed to the wait/kill path.
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
            // Drop closes the pipe — ffmpeg's input loop sees EOF and
            // exits cleanly.
        }

        // Poll for clean exit up to ~3 s.
        let deadline = Instant::now() + Duration::from_millis(3000);
        loop {
            match self.child.try_wait()? {
                Some(status) => {
                    finalized = status.success();
                    break;
                }
                None => {
                    if Instant::now() > deadline {
                        // Polite shutdown stalled — force kill.
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        finalized = false;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
        }

        // If ffmpeg didn't finalize cleanly, surface the tail of stderr
        // to the trace log so failures don't disappear into silence.
        if !finalized {
            if let Some(handle) = self.stderr_thread.take() {
                if let Ok(buf) = handle.join() {
                    let tail = String::from_utf8_lossy(&buf);
                    tracing::warn!(target: "recording",
                        "ffmpeg did not finalize cleanly. Last stderr tail:\n{tail}");
                }
            }
        } else {
            // Still join the thread so the OS handle gets cleaned up.
            if let Some(handle) = self.stderr_thread.take() {
                let _ = handle.join();
            }
        }

        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized,
        })
    }
}

/// Locate `ffprobe` using the same PATH + package-manager fallback the
/// `find_ffmpeg` lookup uses. ffprobe ships next to ffmpeg in every
/// build I've seen, so we just transform the resolved ffmpeg path.
pub fn find_ffprobe() -> Option<PathBuf> {
    let ffmpeg = find_ffmpeg()?;
    // PATH lookup form returns just "ffmpeg" — assume "ffprobe" is on
    // the same PATH.
    if ffmpeg.parent().map(|p| p.as_os_str().is_empty()).unwrap_or(true) {
        return Some(PathBuf::from("ffprobe"));
    }
    // File-path form: swap the filename.
    let mut p = ffmpeg.clone();
    p.set_file_name(if cfg!(target_os = "windows") { "ffprobe.exe" } else { "ffprobe" });
    if p.exists() { Some(p) } else { None }
}

/// Locate the ffmpeg binary. Tries `ffmpeg` (PATH lookup) first, then a
/// few well-known package-manager install paths so a freshly winget /
/// brew / apt-installed ffmpeg works without a shell restart.
pub(crate) fn find_ffmpeg() -> Option<PathBuf> {
    // 1. On PATH via the OS resolver.
    if Command::new("ffmpeg").arg("-version")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
    {
        return Some(PathBuf::from("ffmpeg"));
    }

    // 2. Well-known install locations.
    #[cfg(target_os = "windows")]
    {
        // winget Gyan.FFmpeg drops a versioned dir under WinGet/Packages.
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            let pkg_root = PathBuf::from(local_appdata)
                .join("Microsoft/WinGet/Packages");
            if let Ok(entries) = std::fs::read_dir(&pkg_root) {
                for e in entries.flatten() {
                    let name = e.file_name();
                    if name.to_string_lossy().starts_with("Gyan.FFmpeg") {
                        if let Ok(sub_entries) = std::fs::read_dir(e.path()) {
                            for sub in sub_entries.flatten() {
                                let cand = sub.path().join("bin").join("ffmpeg.exe");
                                if cand.exists() {
                                    return Some(cand);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Also check chocolatey / scoop default locations.
        for p in &[
            "C:/ProgramData/chocolatey/bin/ffmpeg.exe",
            "C:/tools/ffmpeg/bin/ffmpeg.exe",
        ] {
            let pb = PathBuf::from(p);
            if pb.exists() { return Some(pb); }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for p in &[
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
        ] {
            let pb = PathBuf::from(p);
            if pb.exists() { return Some(pb); }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for p in &[
            "/usr/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/snap/bin/ffmpeg",
        ] {
            let pb = PathBuf::from(p);
            if pb.exists() { return Some(pb); }
        }
    }

    None
}

/// Append the platform-appropriate `-framerate N -f <device> -i <source>`
/// flags to an in-progress ffmpeg command.
fn platform_input_args(cmd: &mut Command) {
    let framerate = "30";

    #[cfg(target_os = "windows")]
    {
        // gdigrab is the bundled GDI screen-grab device on Windows. `desktop`
        // captures the entire virtual desktop; switch to `title=<window>`
        // for per-window capture (deferred — main display is the Swift
        // parity behavior).
        cmd.arg("-f").arg("gdigrab")
            .arg("-framerate").arg(framerate)
            .arg("-draw_mouse").arg("1") // include the OS cursor
            .arg("-i").arg("desktop");
    }

    #[cfg(target_os = "macos")]
    {
        // avfoundation: "1" = main display, ":" = no audio input. The
        // exact index of the main display can vary; the Swift impl
        // selected it programmatically through SCShareableContent. ffmpeg
        // accepts `default` as an alias when present (newer macOS), with
        // "1" as the historical convention.
        cmd.arg("-f").arg("avfoundation")
            .arg("-framerate").arg(framerate)
            .arg("-pix_fmt").arg("uyvy422") // avfoundation's native fmt
            .arg("-i").arg("1:");
    }

    #[cfg(target_os = "linux")]
    {
        // x11grab uses the DISPLAY env var. Wayland users need a
        // different backend (pipewire / wf-recorder) — left as a TODO.
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0.0".into());
        cmd.arg("-f").arg("x11grab")
            .arg("-framerate").arg(framerate)
            .arg("-i").arg(display);
    }
}
