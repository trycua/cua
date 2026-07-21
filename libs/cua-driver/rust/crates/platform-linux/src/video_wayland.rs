//! Native Wayland full-desktop video capture.
//!
//! wlroots compositors use `wf-recorder`. GNOME does not expose the wlroots
//! screencopy protocol, so its attested Shell helper supplies compositor-owned
//! PNG frames which are encoded into the same MP4 artifact through FFmpeg.

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use cua_driver_core::video::{VideoBackend, VideoBackendFactory, VideoMetadata};

pub struct WfRecorderVideoBackendFactory;

impl VideoBackendFactory for WfRecorderVideoBackendFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        if let Some(first_frame) = crate::wayland::shell_helper::trusted_screenshot_display() {
            return GnomeShellVideoBackend::start(output_path, first_frame)
                .map(|backend| Box::new(backend) as Box<dyn VideoBackend>);
        }
        WfRecorderVideoBackend::start(output_path)
            .map(|backend| Box::new(backend) as Box<dyn VideoBackend>)
    }
}

struct GnomeShellVideoBackend {
    stop_tx: mpsc::Sender<()>,
    worker: std::thread::JoinHandle<anyhow::Result<()>>,
    output_path: PathBuf,
    started_at: Instant,
}

impl GnomeShellVideoBackend {
    fn start(output_path: &Path, first_frame: Vec<u8>) -> anyhow::Result<Self> {
        let available = Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !available {
            anyhow::bail!("ffmpeg is required for GNOME Wayland video capture");
        }
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let output_path = output_path.to_path_buf();
        let worker_output = output_path.clone();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result = run_gnome_shell_encoder(&worker_output, first_frame, stop_rx, &ready_tx);
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
    output_path: &Path,
    first_frame: Vec<u8>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: &mpsc::SyncSender<Result<(), String>>,
) -> anyhow::Result<()> {
    let mut child = Command::new("ffmpeg")
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
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

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
        let stderr_thread = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut output = Vec::new();
                let _ = stderr.read_to_end(&mut output);
                if output.len() > 4096 {
                    let excess = output.len() - 4096;
                    output.drain(..excess);
                }
                output
            })
        });

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
