//! Native Wayland full-desktop video capture through `wf-recorder`.
//!
//! FFmpeg's Linux input backend is X11-only. On a native Wayland session,
//! `wf-recorder` consumes the compositor's screencopy protocol and writes the
//! same MP4 artifact shape expected by the shared recording session.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_core::video::{VideoBackend, VideoBackendFactory, VideoMetadata};

pub struct WfRecorderVideoBackendFactory;

impl VideoBackendFactory for WfRecorderVideoBackendFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        WfRecorderVideoBackend::start(output_path)
            .map(|backend| Box::new(backend) as Box<dyn VideoBackend>)
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
