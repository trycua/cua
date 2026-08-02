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
use crate::recording_zoom::{generate_zoom_regions, ZoomRegion};
use crate::video_ffmpeg::{find_ffmpeg, find_ffprobe};

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
        Self {
            no_zoom: false,
            default_scale: DEFAULT_ZOOM_SCALE,
        }
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
    let traj =
        load(input_dir).map_err(|e: LoadError| anyhow::anyhow!("load {input_dir:?}: {e}"))?;
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        anyhow::anyhow!(
            "ffmpeg not found on PATH. Install with: \
             winget install Gyan.FFmpeg (Windows), \
             brew install ffmpeg (macOS), \
             or apt install ffmpeg (Linux)."
        )
    })?;

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
        // **Per-frame** smooth zoom via the `zoompan` filter. Earlier
        // attempts using `crop`'s `w`/`h` expressions silently failed
        // because the `crop` filter evaluates w/h **once at init** —
        // only x/y are evaluated per frame. zoompan was designed for
        // this exact use case (Ken Burns-style zoom): its `z`, `x`, `y`
        // expressions all evaluate every output frame.
        //
        // Knobs:
        //   - z (zoom factor): expression from `build_zoom_expression`
        //   - x, y (top-left of source crop window in input coords):
        //         expressions from `build_xy_expressions`
        //   - d=1: produce one output frame per input frame (zoompan's
        //         default is 90 which is a slideshow assumption)
        //   - fps=30: target frame rate
        //   - s=WxH: output size matches input
        let video_w = traj.metadata.video_width as f64;
        let video_h = traj.metadata.video_height as f64;
        let (zoom_expr, x_expr, y_expr) = build_zoompan_expressions(
            &regions,
            video_w,
            video_h,
            traj.metadata.display_scale_factor,
        );
        let expr_dump = input_dir.join("render.expression.txt");
        let _ = std::fs::write(
            &expr_dump,
            format!("z:\n{zoom_expr}\n\nx:\n{x_expr}\n\ny:\n{y_expr}\n"),
        );
        format!(
            "zoompan=z='{z}':x='{x}':y='{y}':d=1:s={w}x{h}:fps=30,\
             pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p",
            z = zoom_expr,
            x = x_expr,
            y = y_expr,
            w = traj.metadata.video_width,
            h = traj.metadata.video_height,
        )
    };

    // Build ffmpeg command.
    let mut cmd = Command::new(&ffmpeg);
    cmd.arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(&traj.metadata.video_path)
        .arg("-vf")
        .arg(&filter_chain)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("medium") // medium is fine for off-line render
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let out = cmd
        .output()
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

/// Build the three `zoompan` expressions (`z`, `x`, `y`) that drive
/// smooth per-frame zoom. zoompan's `time` variable is seconds-since-
/// stream-start; per-region branches use it the same way as the old
/// crop-expression approach.
///
/// Per-region math (in seconds — `time` is the stream-presentation
/// timestamp in seconds; we use `time` because zoompan's `t` variable
/// is overloaded):
///
/// ```text
///   in_t     = clip((time - START)/ZOOM_IN_SEC,  0, 1)
///   out_t    = clip((END - time)/ZOOM_OUT_SEC,   0, 1)
///   phase    = (during zoom-in) in_t → (during hold) 1 → (during zoom-out) out_t
///   ease     = 1 - (1-phase)^5         // quintic ease-out
///   z        = 1 + ease * (SCALE - 1)  // zoompan zoom factor
///   x        = clip(focus_x - iw/(2*z),  0, iw - iw/z)
///   y        = clip(focus_y - ih/(2*z),  0, ih - ih/z)
/// ```
///
/// zoompan semantics: `z` is the zoom factor (1 = no zoom, 2 = 2×),
/// `x` / `y` are the top-left of the source crop window in INPUT
/// coords, source crop size is `iw/z`-by-`ih/z`. Output is always
/// `s=WxH` (set in the filter args).
fn build_zoompan_expressions(
    regions: &[ZoomRegion],
    video_w: f64,
    video_h: f64,
    points_to_pixels: f64,
) -> (String, String, String) {
    // Outside-region defaults: no zoom, anchored at origin (since the
    // crop window IS the full frame at z=1).
    let mut z_expr = "1".to_string();
    let mut x_expr = "0".to_string();
    let mut y_expr = "0".to_string();

    // Iterate in REVERSE — outer `if` covers the FIRST region, nested
    // `else` branches cover later ones, final fallthrough = no-zoom.
    for region in regions.iter().rev() {
        let start_s = region.start_ms / 1000.0;
        let end_s = region.end_ms / 1000.0;
        let (zoom_in_ms, zoom_out_ms) = region.effective_durations();
        let zoom_in_s = (zoom_in_ms / 1000.0).max(1e-3);
        let zoom_out_s = (zoom_out_ms / 1000.0).max(1e-3);
        let in_end_s = start_s + zoom_in_s;
        let out_start_s = end_s - zoom_out_s;

        // Focus in pixels.
        let fx_px = region.focus_x * points_to_pixels;
        let fy_px = region.focus_y * points_to_pixels;
        let peak_scale = region.scale;

        // Phase + ease + scale_at_t expressed in zoompan's `time` var.
        let in_t = format!("max(0,min(1,(time-{start_s})/{zoom_in_s}))");
        let out_t = format!("max(0,min(1,({end_s}-time)/{zoom_out_s}))");
        let phase = format!("if(lt(time,{in_end_s}),{in_t},if(gt(time,{out_start_s}),{out_t},1))");
        let ease = format!("(1-pow(1-({phase}),5))");
        let region_z = format!("(1+{ease}*({peak_scale}-1))");
        // x = clip(focus - iw/(2*z), 0, iw - iw/z)
        let region_x =
            format!("max(0,min({video_w}-{video_w}/{region_z},{fx_px}-{video_w}/(2*{region_z})))");
        let region_y =
            format!("max(0,min({video_h}-{video_h}/{region_z},{fy_px}-{video_h}/(2*{region_z})))");

        z_expr = format!("if(between(time,{start_s},{end_s}),{region_z},{z_expr})");
        x_expr = format!("if(between(time,{start_s},{end_s}),{region_x},{x_expr})");
        y_expr = format!("if(between(time,{start_s},{end_s}),{region_y},{y_expr})");
    }

    (z_expr, x_expr, y_expr)
}

/// Use ffprobe to get the input video duration in ms. Falls back to 0
/// when ffprobe isn't available; the renderer still produces output
/// but the sendcmd tail may stop earlier than the actual frames.
fn probe_duration_ms(video_path: &Path) -> Option<f64> {
    let ffprobe = find_ffprobe()?;
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(video_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let secs: f64 = s.trim().parse().ok()?;
    Some(secs * 1000.0)
}
