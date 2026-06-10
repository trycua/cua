//! ffmpeg-subprocess video backend (Windows + Linux).
//!
//! macOS uses native ScreenCaptureKit in `platform_macos::video_sckit`
//! — that backend doesn't need ffmpeg and doesn't carry the TCC
//! per-binary subprocess gotcha. This file is the cross-platform
//! fallback for OSes where we ship a subprocess pipeline instead.
//!
//! Inputs per OS:
//! - **Windows:** `gdigrab` (full virtual desktop)
//! - **Linux:**   `x11grab` (`$DISPLAY` env var, defaults to `:0.0`)
//!
//! Encoder is `libx264 -preset ultrafast -pix_fmt yuv420p` everywhere.
//! Lifecycle: spawn → caller stays alive → send `q\n` on stdin for a
//! clean shutdown that finalizes the moov atom (force-kill on stall).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::video::{VideoBackend, VideoBackendFactory, VideoMetadata};

pub struct FfmpegVideoBackendFactory;

impl VideoBackendFactory for FfmpegVideoBackendFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        FfmpegVideoBackend::start(output_path).map(|b| Box::new(b) as Box<dyn VideoBackend>)
    }
}

pub struct FfmpegVideoBackend {
    child: Child,
    output_path: PathBuf,
    started_at: Instant,
    /// Drains ffmpeg's stderr so its pipe doesn't fill up and block the
    /// encoder. The collected tail is read in `stop()` for diagnostics
    /// when ffmpeg exits non-zero.
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl FfmpegVideoBackend {
    fn start(output_path: &Path) -> anyhow::Result<Self> {
        let output_path = output_path.to_path_buf();

        let ffmpeg = match find_ffmpeg() {
            Some(p) => p,
            None => anyhow::bail!(
                "ffmpeg not found on PATH. Install with: \
                 winget install Gyan.FFmpeg (Windows) or \
                 apt install ffmpeg (Linux)."
            ),
        };

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
            .arg("-loglevel").arg("error");

        platform_input_args(&mut cmd);

        // yuv420p needs even dimensions; pad rather than crop so the full
        // display stays in frame on odd resolutions (e.g. 1512×949 Win11).
        cmd.arg("-vf").arg("pad=ceil(iw/2)*2:ceil(ih/2)*2");

        cmd.arg("-c:v").arg("libx264")
            .arg("-preset").arg("ultrafast")
            .arg("-pix_fmt").arg("yuv420p")
            .arg("-movflags").arg("+faststart")
            .arg("-g").arg("30")
            .arg(&output_path);

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!("Failed to spawn ffmpeg ({}): {e}", ffmpeg.display())
        })?;

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

        // Fast-fail probe — surface stderr immediately when ffmpeg dies
        // on startup (bad input device, missing codec). Without this the
        // recording session would record a useless empty mp4.
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

        Ok(FfmpegVideoBackend {
            child,
            output_path,
            started_at: Instant::now(),
            stderr_thread,
        })
    }
}

impl VideoBackend for FfmpegVideoBackend {
    fn stop(mut self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        let elapsed = self.started_at.elapsed();
        let finalized;

        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }

        let deadline = Instant::now() + Duration::from_millis(3000);
        loop {
            match self.child.try_wait()? {
                Some(status) => {
                    finalized = status.success();
                    break;
                }
                None => {
                    if Instant::now() > deadline {
                        // Polite shutdown stalled — force kill. mp4 will lack
                        // a moov atom and won't be playable; `finalized:
                        // false` tells the caller.
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        finalized = false;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
        }

        if !finalized {
            if let Some(handle) = self.stderr_thread.take() {
                if let Ok(buf) = handle.join() {
                    let tail = String::from_utf8_lossy(&buf);
                    tracing::warn!(target: "recording",
                        "ffmpeg did not finalize cleanly. Last stderr tail:\n{tail}");
                }
            }
        } else if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }

        Ok(VideoMetadata {
            path: self.output_path,
            duration_ms: elapsed.as_millis() as u64,
            finalized,
        })
    }
}

/// Locate `ffprobe` next to the resolved `ffmpeg`. Used by the recording
/// renderer to probe mp4 duration. ffprobe ships next to ffmpeg in every
/// distro I've seen.
pub fn find_ffprobe() -> Option<PathBuf> {
    let ffmpeg = find_ffmpeg()?;
    if ffmpeg.parent().map(|p| p.as_os_str().is_empty()).unwrap_or(true) {
        return Some(PathBuf::from("ffprobe"));
    }
    let mut p = ffmpeg.clone();
    p.set_file_name(if cfg!(target_os = "windows") { "ffprobe.exe" } else { "ffprobe" });
    if p.exists() { Some(p) } else { None }
}

/// Locate the ffmpeg binary (PATH first, then well-known install dirs).
/// Public so platform video backends (e.g. the Linux Wayland screencopy
/// pipeline) can reuse the same resolution logic.
pub fn find_ffmpeg() -> Option<PathBuf> {
    if Command::new("ffmpeg").arg("-version")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
    {
        return Some(PathBuf::from("ffmpeg"));
    }

    #[cfg(target_os = "windows")]
    {
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
        for p in &[
            "C:/ProgramData/chocolatey/bin/ffmpeg.exe",
            "C:/tools/ffmpeg/bin/ffmpeg.exe",
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

    #[cfg(target_os = "macos")]
    {
        // ffmpeg backend isn't wired on macOS, but recording_render still
        // calls find_ffprobe to inspect existing mp4s. Keep the macOS
        // probe so that path keeps working when ffmpeg is installed.
        for p in &[
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
        ] {
            let pb = PathBuf::from(p);
            if pb.exists() { return Some(pb); }
        }
    }

    None
}

fn platform_input_args(cmd: &mut Command) {
    let framerate = "30";

    #[cfg(target_os = "windows")]
    {
        cmd.arg("-f").arg("gdigrab")
            .arg("-framerate").arg(framerate)
            .arg("-draw_mouse").arg("1")
            .arg("-i").arg("desktop");
    }

    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0.0".into());
        cmd.arg("-f").arg("x11grab")
            .arg("-framerate").arg(framerate)
            .arg("-i").arg(display);
    }

    // macOS not wired here — the macOS factory is `SckitVideoBackendFactory`
    // in platform-macos and the ffmpeg backend is never registered.
    #[cfg(target_os = "macos")]
    {
        let _ = framerate;
        let _ = cmd;
    }
}
