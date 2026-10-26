//! Screen recording of a Space: frames from the computer service, encoded
//! to MP4 by `ffmpeg` on this machine.
//!
//! A recorder takes screenshots at a fixed rate through
//! `ComputerService.Screenshot` (so it works for any guest OS the computer
//! service covers: Linux, macOS, Windows), pipes them to
//! `ffmpeg -f image2pipe ... libx264` and finishes the file when stopped.
//! `ffmpeg` is looked up in `$CUA_FFMPEG`, then `PATH`; without it
//! starting fails with [`Error::FeatureUnsupported`]-style text naming it.

use crate::{
    client::SpacesdClient,
    computer::ScreenshotOptions,
    error::{Error, Result},
};
use cua_proto::env::v1 as pb;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// What to record.
#[derive(Clone, Debug)]
pub struct RecordOptions {
    /// Frames per second (default 5, at most 30).
    pub fps: u32,
    /// Where the MP4 goes.
    pub out: PathBuf,
    /// Long-edge cap of a frame in pixels (0: native).
    pub max_dimension: u32,
    /// Draw the cursor in frames.
    pub include_cursor: bool,
}

impl RecordOptions {
    pub fn new(out: impl Into<PathBuf>) -> Self {
        Self {
            fps: 5,
            out: out.into(),
            max_dimension: 0,
            include_cursor: true,
        }
    }
}

/// A finished recording.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub path: PathBuf,
    pub frames: u64,
    pub seconds: f64,
    pub bytes: u64,
}

/// A running recording.
pub struct ScreenRecording {
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    started: Instant,
    task: tokio::task::JoinHandle<std::result::Result<(), String>>,
    out: PathBuf,
}

/// The `ffmpeg` to use.
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CUA_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
        .or_else(|| {
            [
                "/opt/homebrew/bin/ffmpeg",
                "/usr/local/bin/ffmpeg",
                "/usr/bin/ffmpeg",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        })
}

/// The ffmpeg arguments for frames on stdin (PNG or JPEG), to `out`.
pub fn ffmpeg_args(fps: u32, out: &Path) -> Vec<String> {
    let fps = fps.clamp(1, 30);
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "image2pipe",
        "-framerate",
        &fps.to_string(),
        "-i",
        "-",
        "-vf",
        "scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-movflags",
        "+faststart",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([out.display().to_string()])
    .collect()
}

impl SpacesdClient {
    /// Starts recording the primary display to `opts.out`.
    pub async fn record_screen(&self, opts: RecordOptions) -> Result<ScreenRecording> {
        let ffmpeg = find_ffmpeg().ok_or_else(|| {
            Error::Protocol(
                "recording needs ffmpeg on this machine (install it, or set CUA_FFMPEG)".into(),
            )
        })?;
        if let Some(p) = opts.out.parent() {
            std::fs::create_dir_all(p).map_err(|e| Error::Protocol(e.to_string()))?;
        }
        let mut child = tokio::process::Command::new(ffmpeg)
            .args(ffmpeg_args(opts.fps, &opts.out))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Protocol(format!("starting ffmpeg: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let c = self.clone();
        let (stop2, frames2) = (stop.clone(), frames.clone());
        let period = Duration::from_millis(1000 / u64::from(opts.fps.clamp(1, 30)));
        let shot = ScreenshotOptions {
            format: pb::ImageFormat::Png,
            max_dimension: opts.max_dimension,
            include_cursor: opts.include_cursor,
            ..Default::default()
        };
        let task = tokio::spawn(async move {
            let mut next = Instant::now();
            // A first frame always, so a stop right away still has a video.
            loop {
                match c.screenshot(shot.clone()).await {
                    Ok(s) => {
                        if stdin.write_all(&s.image).await.is_err() {
                            break;
                        }
                        frames2.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(e) => {
                        drop(stdin);
                        let _ = child.wait().await;
                        return Err(format!("screenshot: {e}"));
                    }
                }
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                next += period;
                let now = Instant::now();
                if next > now {
                    tokio::time::sleep(next - now).await;
                } else {
                    next = now;
                }
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
            }
            drop(stdin);
            let status = child.wait().await.map_err(|e| e.to_string())?;
            if status.success() {
                Ok(())
            } else {
                use tokio::io::AsyncReadExt;
                let mut err = String::new();
                let mut stderr = stderr;
                let _ = stderr.read_to_string(&mut err).await;
                Err(format!("ffmpeg exited with {status}: {}", err.trim()))
            }
        });
        Ok(ScreenRecording {
            stop,
            frames,
            started: Instant::now(),
            task,
            out: opts.out,
        })
    }
}

impl ScreenRecording {
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::SeqCst)
    }

    /// Ends the recording and returns the finished file.
    pub async fn stop(self) -> Result<Recorded> {
        self.stop.store(true, Ordering::SeqCst);
        let seconds = self.started.elapsed().as_secs_f64();
        match self.task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(Error::Protocol(e)),
            Err(e) => return Err(Error::Protocol(e.to_string())),
        }
        let bytes = std::fs::metadata(&self.out).map(|m| m.len()).unwrap_or(0);
        Ok(Recorded {
            path: self.out,
            frames: self.frames.load(Ordering::SeqCst),
            seconds,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_reads_frames_from_stdin_at_the_rate() {
        let a = ffmpeg_args(5, Path::new("/tmp/x.mp4"));
        let i = a.iter().position(|s| s == "-framerate").unwrap();
        assert_eq!(a[i + 1], "5");
        assert!(a.contains(&"image2pipe".to_string()) && a.last().unwrap() == "/tmp/x.mp4");
        assert_eq!(ffmpeg_args(500, Path::new("o"))[7], "30");
        assert_eq!(ffmpeg_args(0, Path::new("o"))[7], "1");
    }
}
