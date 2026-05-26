//! Zoom-on-click renderer. Reads a recording directory produced by
//! `start_recording`, builds an ffmpeg filter graph that applies the
//! zoom curve from `recording_zoom`, and writes a new MP4.
//!
//! Cross-platform by construction: the only OS dep is the ffmpeg
//! subprocess, which is already cross-platform.
//!
//! Reference: `libs/cua-driver/swift/Sources/CuaDriverCore/Recording/Render/RecordingRenderer.swift`
//!
//! ## Why a sendcmd file instead of one giant filter expression
//!
//! The naive approach is to fold the entire zoom timeline into one big
//! `crop=…:if(between(t,t1,t2),...)` expression chain. ffmpeg's
//! expression parser has practical depth limits (varies by build, ~50-
//! 100 nests) and debugging a stringified curve is painful.
//!
//! The cleaner approach is `sendcmd`: pre-compute per-frame
//! `(scale, focus_x, focus_y)` in Rust, write timed `crop` + `scale`
//! parameter changes into a sendcmd file, and let ffmpeg apply them in
//! sequence. The sendcmd file is human-readable for verification.
//!
//! ## What we emit
//!
//! Filter chain: `sendcmd=f=<file>,crop=w=W:h=H:x=X:y=Y,scale=outW:outH`
//! plus the standard `pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p`
//! tail for codec compatibility.
//!
//! `sendcmd` issues commands like `crop w 800` at exact timestamps. We
//! sample the zoom curve at 30 Hz (matching the recorded framerate)
//! and write one command set per sample — produces smooth animation
//! since ffmpeg internally interpolates between consecutive frame
//! updates.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::recording_loader::{load, LoadError};
use crate::recording_zoom::{
    generate_zoom_regions, sample_curve, CursorSample, ZoomRegion,
};
use crate::video::{find_ffmpeg, find_ffprobe};

/// Sampling rate for the zoom curve. ffmpeg reads sendcmd updates at
/// each timestamp and holds the value until the next one, so 30 Hz
/// matches the 30 fps capture and yields one update per recorded frame.
const ZOOM_SAMPLE_HZ: f64 = 30.0;

/// Default zoom magnification.
const DEFAULT_ZOOM_SCALE: f64 = 2.0;

#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Skip the zoom curve and re-encode the input as-is. Useful as a
    /// baseline (same resolution, same duration, no visual change) to
    /// verify the reader/writer pipeline independent of the zoom math.
    pub no_zoom: bool,
    /// Zoom scale at the peak of each region. `2.0` is 2× magnification.
    pub default_scale: f64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { no_zoom: false, default_scale: DEFAULT_ZOOM_SCALE }
    }
}

#[derive(Debug)]
pub struct RenderResult {
    pub output_path: PathBuf,
    pub input_duration_ms: f64,
    pub zoom_region_count: usize,
}

/// Render a recording directory at `input_dir` into a polished mp4 at
/// `output_path`.
pub fn render(
    input_dir: &Path,
    output_path: &Path,
    opts: &RenderOptions,
) -> anyhow::Result<RenderResult> {
    let traj = load(input_dir)
        .map_err(|e: LoadError| anyhow::anyhow!("load {input_dir:?}: {e}"))?;
    let ffmpeg = find_ffmpeg()
        .ok_or_else(|| anyhow::anyhow!(
            "ffmpeg not found on PATH. Install with: \
             winget install Gyan.FFmpeg (Windows), \
             brew install ffmpeg (macOS), \
             or apt install ffmpeg (Linux)."))?;

    let in_dur = probe_duration_ms(&traj.metadata.video_path).unwrap_or(0.0);

    // Build the filter chain. The two paths diverge:
    //   - `no_zoom`: just transcode through codec-compatibility pad+pix_fmt
    //   - normal:    generate sendcmd file, prepend crop+scale chain
    let regions = if opts.no_zoom {
        Vec::new()
    } else {
        generate_zoom_regions(&traj.clicks, opts.default_scale)
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let filter_chain = if regions.is_empty() {
        // Pure transcode path.
        "pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p".to_string()
    } else {
        // Write the sendcmd file alongside the input dir so it's
        // inspectable after-the-fact. Renamed each render so successive
        // runs don't stomp each other.
        let sendcmd_path = input_dir.join("render.sendcmd");
        write_sendcmd(
            &sendcmd_path,
            &regions,
            &traj.cursor_samples,
            in_dur,
            traj.metadata.video_width as f64,
            traj.metadata.video_height as f64,
            traj.metadata.display_scale_factor,
        )?;
        // Filter chain:
        //   - sendcmd=f=<file>: drives crop@c at timed offsets
        //   - crop@c: extracts the zoom region (params updated by sendcmd)
        //   - scale=<outW>:<outH>: stretches the crop back to full frame
        //   - pad/format: codec-compat tail
        format!(
            "sendcmd=f='{}',crop@c=w={w}:h={h}:x=0:y=0,scale={w}:{h},\
             pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p",
            // ffmpeg filter args need backslashes escaped on Windows.
            sendcmd_path.to_string_lossy().replace('\\', "/").replace(':', "\\:"),
            w = traj.metadata.video_width,
            h = traj.metadata.video_height,
        )
    };

    // Build ffmpeg command.
    let mut cmd = Command::new(&ffmpeg);
    cmd.arg("-y")
        .arg("-loglevel").arg("error")
        .arg("-i").arg(&traj.metadata.video_path)
        .arg("-vf").arg(&filter_chain)
        .arg("-c:v").arg("libx264")
        .arg("-preset").arg("medium") // medium is fine for off-line render
        .arg("-pix_fmt").arg("yuv420p")
        .arg("-movflags").arg("+faststart")
        .arg(output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let out = cmd.output()
        .map_err(|e| anyhow::anyhow!("spawn ffmpeg: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("ffmpeg failed (exit {}): {stderr}", out.status);
    }

    Ok(RenderResult {
        output_path: output_path.to_owned(),
        input_duration_ms: in_dur,
        zoom_region_count: regions.len(),
    })
}

/// Compose the sendcmd file: at each tick, emit `crop@c w X; crop@c h X;
/// crop@c x X; crop@c y X;`. ffmpeg holds each value until the next tick.
fn write_sendcmd(
    sendcmd_path: &Path,
    regions: &[ZoomRegion],
    cursor_samples: &[CursorSample],
    duration_ms: f64,
    video_w: f64,
    video_h: f64,
    points_to_pixels: f64,
) -> anyhow::Result<()> {
    let mut buf = String::new();
    let tick_ms = 1000.0 / ZOOM_SAMPLE_HZ;
    let mut t = 0.0;
    while t <= duration_ms + tick_ms {
        let (scale, fx_pts, fy_pts) = sample_curve(t, regions, cursor_samples);
        // Convert cursor-space points → video pixels (Retina factor).
        let fx_px = fx_pts * points_to_pixels;
        let fy_px = fy_pts * points_to_pixels;
        let safe_scale = scale.max(1.0);
        let crop_w = (video_w / safe_scale).max(2.0);
        let crop_h = (video_h / safe_scale).max(2.0);
        let half_w = crop_w / 2.0;
        let half_h = crop_h / 2.0;
        // Slide the crop in so we don't run off the edge.
        let crop_x = (fx_px - half_w).clamp(0.0, video_w - crop_w);
        let crop_y = (fy_px - half_h).clamp(0.0, video_h - crop_h);

        // ffmpeg sendcmd timestamp is in seconds.
        let t_sec = t / 1000.0;
        // One line per tick with all four updates. Using integer pixel
        // values keeps ffmpeg's parser happy (its filter args don't
        // accept arbitrary decimal precision in expression-free mode).
        buf.push_str(&format!(
            "{t_sec:.4} crop@c w {cw}, crop@c h {ch}, crop@c x {cx}, crop@c y {cy};\n",
            cw = crop_w.round() as i64,
            ch = crop_h.round() as i64,
            cx = crop_x.round() as i64,
            cy = crop_y.round() as i64,
        ));
        t += tick_ms;
    }

    std::fs::write(sendcmd_path, buf.as_bytes())?;
    Ok(())
}

/// Use ffprobe to get the input video duration in ms. Falls back to 0
/// when ffprobe isn't available; the renderer still produces output
/// but the sendcmd tail may stop earlier than the actual frames.
fn probe_duration_ms(video_path: &Path) -> Option<f64> {
    let ffprobe = find_ffprobe()?;
    let out = Command::new(ffprobe)
        .args(["-v", "error",
               "-show_entries", "format=duration",
               "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(video_path)
        .output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout);
    let secs: f64 = s.trim().parse().ok()?;
    Some(secs * 1000.0)
}

