// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Codec-neutral video encoding seam.
//!
//! Capture backends that produce CPU BGRA frames (Linux XShm, any future
//! PipeWire path) hand them to an [`EncodingPipeline`], which owns one
//! [`FrameEncoder`] on a dedicated worker thread. The encoder itself comes
//! from a [`FrameEncoderFactory`]; today that is OpenH264 (software) on Linux
//! and Windows, and VideoToolbox stays integrated in the macOS capture path.
//! The `cua-media-codec` crate plugs hardware encoders (NVENC, VA-API, QSV,
//! AMF, Media Foundation) in by implementing these two traits.
//!
//! Pipeline rules:
//! - One pending raw frame; a newer frame replaces an unencoded one, so the
//!   capture thread never waits on the encoder.
//! - A keyframe request with no new frame re-encodes the last frame as an IDR,
//!   so a static screen still answers a keyframe request (keyframe on attach).
//! - A dimension change recreates the encoder, advances the codec epoch and
//!   forces an IDR.
//! - Bitrate and frame-rate changes apply live.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use cua_spacesd_provider_api::{CaptureEvent, CaptureSink, OwnedFrame, PixelFormat};

/// Encoder construction parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Encoded width; even.
    pub width: u32,
    /// Encoded height; even.
    pub height: u32,
    pub max_fps: u16,
    pub bitrate_kbps: u32,
    /// Prefer gradual intra refresh over periodic IDRs where supported.
    pub intra_refresh: bool,
}

/// One CPU frame handed to an encoder: top-down BGRA.
#[derive(Debug, Clone, Copy)]
pub struct RawVideoFrame<'a> {
    pub bgra: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// Media-clock capture time.
    pub capture_us: u64,
}

/// Encoded output: one Annex B access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVideo {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// The encoder restarted internally (for example a runtime fallback to
    /// another backend): decoders must reset, so the pipeline starts a new
    /// codec epoch. Must coincide with a keyframe.
    pub restart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderError(pub String);

impl std::fmt::Display for EncoderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EncoderError {}

/// A stateful video encoder. Implementations must emit SPS/PPS with every
/// keyframe, use no B-frames, and honour `force_keyframe` on the very next
/// output.
pub trait FrameEncoder: Send {
    fn name(&self) -> &'static str;

    /// Encode one frame. `Ok(None)` means the encoder skipped it (rate
    /// control) and nothing is sent.
    fn encode(
        &mut self,
        frame: &RawVideoFrame<'_>,
        force_keyframe: bool,
    ) -> Result<Option<EncodedVideo>, EncoderError>;

    fn set_bitrate_kbps(&mut self, _kbps: u32) {}

    fn set_max_fps(&mut self, _fps: u16) {}
}

/// Creates encoders; one factory per backend.
pub trait FrameEncoderFactory: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn FrameEncoder>, EncoderError>;
}

/// A captured BGRA frame owned by the pipeline.
#[derive(Debug, Clone)]
pub struct OwnedRawFrame {
    pub bgra: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub capture_us: u64,
}

impl OwnedRawFrame {
    fn view(&self) -> RawVideoFrame<'_> {
        RawVideoFrame {
            bgra: &self.bgra,
            width: self.width,
            height: self.height,
            stride: self.stride,
            capture_us: self.capture_us,
        }
    }
}

#[derive(Default)]
struct Slot {
    pending: Option<OwnedRawFrame>,
    last: Option<OwnedRawFrame>,
    force_keyframe: bool,
    bitrate_kbps: Option<u32>,
    max_fps: Option<u16>,
    stopped: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

/// Owns one encoder on a worker thread and publishes H.264 frames to a
/// capture sink.
pub struct EncodingPipeline {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    encoder_name: &'static str,
}

impl EncodingPipeline {
    pub fn start(
        factory: Arc<dyn FrameEncoderFactory>,
        max_fps: u16,
        bitrate_kbps: u32,
        sink: Arc<dyn CaptureSink>,
    ) -> Self {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                force_keyframe: true,
                ..Slot::default()
            }),
            wake: Condvar::new(),
        });
        let encoder_name = factory.name();
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name(format!("cua-env-encode-{encoder_name}"))
            .spawn(move || encode_loop(worker_shared, factory, max_fps, bitrate_kbps, sink))
            .expect("spawn encoder thread");
        Self {
            shared,
            worker: Mutex::new(Some(worker)),
            encoder_name,
        }
    }

    pub fn encoder_name(&self) -> &'static str {
        self.encoder_name
    }

    fn with_slot(&self, update: impl FnOnce(&mut Slot)) {
        let mut slot = self
            .shared
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut slot);
        self.shared.wake.notify_one();
    }

    /// Queue a frame, replacing any frame the encoder has not started yet.
    pub fn submit(&self, frame: OwnedRawFrame) {
        self.with_slot(|slot| slot.pending = Some(frame));
    }

    /// Make the next output an IDR. Re-encodes the last frame when no new
    /// frame arrives, so a static target still answers.
    pub fn request_keyframe(&self) {
        self.with_slot(|slot| slot.force_keyframe = true);
    }

    pub fn set_bitrate_kbps(&self, kbps: u32) {
        self.with_slot(|slot| slot.bitrate_kbps = Some(kbps));
    }

    pub fn set_max_fps(&self, fps: u16) {
        self.with_slot(|slot| slot.max_fps = Some(fps));
    }

    pub fn stop(&self) {
        self.with_slot(|slot| slot.stopped = true);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            if worker.thread().id() != std::thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for EncodingPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Encoder counters, logged once when the pipeline stops, so a low frame
/// rate can be placed (capture or encoder).
struct EncodeStats {
    started: Instant,
    frames: u64,
    total_us: u64,
    max_us: u32,
}

impl Default for EncodeStats {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            frames: 0,
            total_us: 0,
            max_us: 0,
        }
    }
}

impl EncodeStats {
    fn record(&mut self, encode_us: u32) {
        self.frames += 1;
        self.total_us += u64::from(encode_us);
        self.max_us = self.max_us.max(encode_us);
    }
}

impl Drop for EncodeStats {
    fn drop(&mut self) {
        if self.frames == 0 {
            return;
        }
        let seconds = self.started.elapsed().as_secs_f64();
        tracing::info!(
            target: "cua_spacesd_client::encoder",
            seconds = format!("{seconds:.1}"),
            frames = self.frames,
            fps = format!("{:.1}", self.frames as f64 / seconds.max(0.001)),
            avg_ms = format!("{:.1}", self.total_us as f64 / self.frames as f64 / 1000.0),
            max_ms = format!("{:.1}", f64::from(self.max_us) / 1000.0),
            "encoder stopped"
        );
    }
}

fn encode_loop(
    shared: Arc<Shared>,
    factory: Arc<dyn FrameEncoderFactory>,
    mut max_fps: u16,
    mut bitrate_kbps: u32,
    sink: Arc<dyn CaptureSink>,
) {
    let mut encoder: Option<(Box<dyn FrameEncoder>, u32, u32)> = None;
    let mut codec_epoch = 0u64;
    let mut failures = 0u32;
    let mut stats = EncodeStats::default();
    loop {
        let (frame, force_keyframe, new_bitrate, new_fps) = {
            let mut slot = shared
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if slot.stopped {
                    return;
                }
                let replay = slot.force_keyframe && slot.pending.is_none() && slot.last.is_some();
                if slot.pending.is_some() || replay {
                    break;
                }
                slot = shared
                    .wake
                    .wait(slot)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            let frame = match slot.pending.take() {
                Some(frame) => {
                    slot.last = Some(frame.clone());
                    frame
                }
                None => slot.last.clone().expect("replay has a last frame"),
            };
            let force = std::mem::take(&mut slot.force_keyframe);
            (frame, force, slot.bitrate_kbps.take(), slot.max_fps.take())
        };
        if let Some(kbps) = new_bitrate {
            bitrate_kbps = kbps;
        }
        if let Some(fps) = new_fps {
            max_fps = fps;
        }
        let width = frame.width & !1;
        let height = frame.height & !1;
        if width == 0 || height == 0 {
            continue;
        }
        let mut force_keyframe = force_keyframe;
        if encoder
            .as_ref()
            .is_none_or(|(_, w, h)| *w != width || *h != height)
        {
            let config = EncoderConfig {
                width,
                height,
                max_fps,
                bitrate_kbps,
                intra_refresh: false,
            };
            match factory.create(&config) {
                Ok(created) => {
                    encoder = Some((created, width, height));
                    codec_epoch += 1;
                    force_keyframe = true;
                }
                Err(error) => {
                    failures += 1;
                    tracing::warn!(target: "cua_spacesd_client::encoder", %error, failures, "encoder creation failed");
                    sink.on_event(CaptureEvent::Suspended("encoder_failed".into()));
                    encoder = None;
                    continue;
                }
            }
        }
        let (active, _, _) = encoder.as_mut().expect("encoder exists");
        if let Some(kbps) = new_bitrate {
            active.set_bitrate_kbps(kbps);
        }
        if let Some(fps) = new_fps {
            active.set_max_fps(fps);
        }
        let view = RawVideoFrame {
            width,
            height,
            ..frame.view()
        };
        let started = Instant::now();
        match active.encode(&view, force_keyframe) {
            Ok(Some(encoded)) => {
                failures = 0;
                if encoded.restart {
                    codec_epoch += 1;
                }
                let encode_us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
                stats.record(encode_us);
                sink.on_event(CaptureEvent::Frame(OwnedFrame {
                    bytes: Arc::from(encoded.data),
                    format: PixelFormat::H264AnnexB,
                    width_px: width,
                    height_px: height,
                    bytes_per_row: None,
                    capture_timestamp_us: frame.capture_us,
                    encode_duration_us: Some(encode_us),
                    codec_epoch,
                    keyframe: encoded.keyframe,
                }));
            }
            Ok(None) => {
                if force_keyframe {
                    // Keep the request pending until an IDR actually leaves.
                    let mut slot = shared
                        .slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    slot.force_keyframe = true;
                }
            }
            Err(error) => {
                failures += 1;
                tracing::warn!(target: "cua_spacesd_client::encoder", %error, failures, "encode failed; recreating encoder");
                encoder = None;
                let mut slot = shared
                    .slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                slot.force_keyframe = true;
                if failures >= 3 {
                    drop(slot);
                    sink.on_event(CaptureEvent::Suspended("encoder_failed".into()));
                }
            }
        }
    }
}

/// Convert top-down BGRA to planar I420 (BT.601, limited range). Width and
/// height must be even. Returns (Y, U, V).
pub fn bgra_to_i420(frame: &RawVideoFrame<'_>) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let stride = frame.stride as usize;
    let mut y_plane = vec![0u8; width * height];
    let mut u_plane = vec![0u8; (width / 2) * (height / 2)];
    let mut v_plane = vec![0u8; (width / 2) * (height / 2)];
    for row in 0..height {
        let source = &frame.bgra[row * stride..row * stride + width * 4];
        let luma = &mut y_plane[row * width..(row + 1) * width];
        for (column, pixel) in source.as_chunks::<4>().0.iter().enumerate() {
            let (b, g, r) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );
            luma[column] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
        }
    }
    for row in 0..height / 2 {
        let top = &frame.bgra[(2 * row) * stride..];
        let bottom = &frame.bgra[(2 * row + 1) * stride..];
        for column in 0..width / 2 {
            let mut sums = [0i32; 3];
            for source in [top, bottom] {
                for offset in [0usize, 4] {
                    let index = column * 8 + offset;
                    sums[0] += i32::from(source[index]);
                    sums[1] += i32::from(source[index + 1]);
                    sums[2] += i32::from(source[index + 2]);
                }
            }
            let (b, g, r) = (sums[0] / 4, sums[1] / 4, sums[2] / 4);
            let index = row * (width / 2) + column;
            u_plane[index] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            v_plane[index] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
    (y_plane, u_plane, v_plane)
}

/// Downscale a tightly packed BGRA frame with nearest-neighbour sampling.
pub fn downscale_bgra(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    source_stride: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut output = vec![0u8; (width * height * 4) as usize];
    for row in 0..height {
        let source_row = (u64::from(row) * u64::from(source_height) / u64::from(height)) as usize;
        for column in 0..width {
            let source_column =
                (u64::from(column) * u64::from(source_width) / u64::from(width)) as usize;
            let from = source_row * source_stride as usize + source_column * 4;
            let to = ((row * width + column) * 4) as usize;
            output[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    output
}

/// Fit `width`x`height` inside a long-edge limit, preserving aspect ratio and
/// rounding to even dimensions. `max_dimension == 0` means no limit.
pub fn fit_even(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    let (mut fit_width, mut fit_height) = (width, height);
    let long = width.max(height);
    if max_dimension > 0 && long > max_dimension {
        fit_width = (u64::from(width) * u64::from(max_dimension) / u64::from(long)) as u32;
        fit_height = (u64::from(height) * u64::from(max_dimension) / u64::from(long)) as u32;
    }
    ((fit_width & !1).max(2), (fit_height & !1).max(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    #[test]
    fn i420_conversion_matches_bt601_reference_colors() {
        // Pure red, green, blue and white 2x2 blocks.
        let colors: [[u8; 4]; 4] = [
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [255, 255, 255, 255],
        ];
        for color in colors {
            let bgra = color.repeat(4);
            let frame = RawVideoFrame {
                bgra: &bgra,
                width: 2,
                height: 2,
                stride: 8,
                capture_us: 0,
            };
            let (y, u, v) = bgra_to_i420(&frame);
            let (b, g, r) = (
                f64::from(color[0]),
                f64::from(color[1]),
                f64::from(color[2]),
            );
            let expected_y = 16.0 + 0.257 * r + 0.504 * g + 0.098 * b;
            let expected_u = 128.0 - 0.148 * r - 0.291 * g + 0.439 * b;
            let expected_v = 128.0 + 0.439 * r - 0.368 * g - 0.071 * b;
            assert!((f64::from(y[0]) - expected_y).abs() <= 1.5, "{color:?} y");
            assert!((f64::from(u[0]) - expected_u).abs() <= 1.5, "{color:?} u");
            assert!((f64::from(v[0]) - expected_v).abs() <= 1.5, "{color:?} v");
        }
    }

    #[test]
    fn fit_even_preserves_aspect_and_parity() {
        assert_eq!(fit_even(1920, 1080, 0), (1920, 1080));
        assert_eq!(fit_even(1921, 1081, 0), (1920, 1080));
        assert_eq!(fit_even(1920, 1080, 960), (960, 540));
        assert_eq!(fit_even(1080, 1920, 1280), (720, 1280));
    }

    struct CountingEncoder {
        frames: Arc<AtomicU32>,
    }

    impl FrameEncoder for CountingEncoder {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn encode(
            &mut self,
            _frame: &RawVideoFrame<'_>,
            force_keyframe: bool,
        ) -> Result<Option<EncodedVideo>, EncoderError> {
            self.frames.fetch_add(1, Ordering::SeqCst);
            let mut data = vec![0, 0, 0, 1, 0x41, 0];
            if force_keyframe {
                data = vec![0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 0];
            }
            Ok(Some(EncodedVideo {
                data,
                keyframe: force_keyframe,
                restart: false,
            }))
        }
    }

    struct CountingFactory {
        created: Arc<AtomicU32>,
        frames: Arc<AtomicU32>,
    }

    impl FrameEncoderFactory for CountingFactory {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn create(&self, _config: &EncoderConfig) -> Result<Box<dyn FrameEncoder>, EncoderError> {
            self.created.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(CountingEncoder {
                frames: self.frames.clone(),
            }))
        }
    }

    #[derive(Default)]
    struct Collect(Mutex<Vec<OwnedFrame>>);

    impl CaptureSink for Collect {
        fn on_event(&self, event: CaptureEvent) {
            if let CaptureEvent::Frame(frame) = event {
                self.0.lock().unwrap().push(frame);
            }
        }
    }

    fn raw(width: u32, height: u32) -> OwnedRawFrame {
        OwnedRawFrame {
            bgra: Arc::from(vec![0u8; (width * height * 4) as usize]),
            width,
            height,
            stride: width * 4,
            capture_us: 1,
        }
    }

    fn wait_for(collect: &Collect, count: usize) -> Vec<OwnedFrame> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let frames = collect.0.lock().unwrap().clone();
            if frames.len() >= count || Instant::now() > deadline {
                return frames;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pipeline_starts_with_idr_replays_on_request_and_restarts_on_resize() {
        let created = Arc::new(AtomicU32::new(0));
        let frames = Arc::new(AtomicU32::new(0));
        let collect = Arc::new(Collect::default());
        let pipeline = EncodingPipeline::start(
            Arc::new(CountingFactory {
                created: created.clone(),
                frames: frames.clone(),
            }),
            30,
            2000,
            collect.clone(),
        );
        pipeline.submit(raw(64, 48));
        let first = wait_for(&collect, 1);
        assert!(first[0].keyframe);
        assert_eq!(first[0].codec_epoch, 1);
        pipeline.submit(raw(64, 48));
        let second = wait_for(&collect, 2);
        assert!(!second[1].keyframe);
        // Static screen: a keyframe request re-encodes the last frame.
        pipeline.request_keyframe();
        let third = wait_for(&collect, 3);
        assert!(third[2].keyframe);
        assert_eq!(third[2].codec_epoch, 1);
        // Resize: new encoder, new epoch, forced IDR; odd sizes are cropped.
        pipeline.submit(raw(81, 61));
        let fourth = wait_for(&collect, 4);
        assert!(fourth[3].keyframe);
        assert_eq!(fourth[3].codec_epoch, 2);
        assert_eq!((fourth[3].width_px, fourth[3].height_px), (80, 60));
        assert_eq!(created.load(Ordering::SeqCst), 2);
        pipeline.stop();
    }
}
