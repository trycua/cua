// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Bounded software H.264 encoding for Windows capture frames.
//!
//! WGC remains the owner of BGRA capture. A one-frame mailbox isolates capture
//! from CPU encoding pressure, and OpenH264 emits one Annex-B access unit per
//! RCDP frame. Replacement, resize, and explicit recovery all force an IDR.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use cua_spacesd_provider_api::{
    CaptureEvent, CaptureSink, OwnedFrame, PixelFormat, ProviderError, ProviderErrorCode,
};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use openh264::OpenH264API;

#[derive(Default)]
struct EncoderQueue {
    latest: Option<OwnedFrame>,
    last_submitted: Option<OwnedFrame>,
    force_keyframe: bool,
    closed: bool,
}

struct EncoderMailbox {
    state: Mutex<EncoderQueue>,
    ready: Condvar,
}

impl EncoderMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(EncoderQueue::default()),
            ready: Condvar::new(),
        }
    }

    fn submit(&self, frame: OwnedFrame) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.last_submitted = Some(frame.clone());
        if state.latest.replace(frame).is_some() {
            state.force_keyframe = true;
        }
        self.ready.notify_one();
    }

    fn request_keyframe(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.force_keyframe = true;
        if state.latest.is_none() {
            state.latest = state.last_submitted.clone();
        }
        self.ready.notify_one();
    }

    fn take(&self) -> Option<(OwnedFrame, bool)> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.latest.is_none() && !state.closed {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let frame = state.latest.take()?;
        let force = std::mem::take(&mut state.force_keyframe);
        Some((frame, force))
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        state.latest = None;
        state.last_submitted = None;
        self.ready.notify_one();
    }
}

pub(super) struct WindowsH264Encoder {
    sink: Arc<dyn CaptureSink>,
    mailbox: Arc<EncoderMailbox>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl WindowsH264Encoder {
    pub(super) fn start(
        max_fps: u16,
        target_bitrate_kbps: Option<u32>,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Arc<Self>, ProviderError> {
        let mailbox = Arc::new(EncoderMailbox::new());
        let worker_mailbox = mailbox.clone();
        let worker_sink = sink.clone();
        let worker = std::thread::Builder::new()
            .name("rcdp-windows-h264".into())
            .spawn(move || encoder_loop(max_fps, target_bitrate_kbps, worker_sink, worker_mailbox))
            .map_err(|error| {
                ProviderError::new(
                    ProviderErrorCode::CaptureFailed,
                    format!("failed to start OpenH264 worker: {error}"),
                )
            })?;
        Ok(Arc::new(Self {
            sink,
            mailbox,
            worker: Mutex::new(Some(worker)),
        }))
    }

    pub(super) fn request_keyframe(&self) {
        self.mailbox.request_keyframe();
    }

    pub(super) fn stop(&self) {
        self.mailbox.close();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl CaptureSink for WindowsH264Encoder {
    fn on_event(&self, event: CaptureEvent) {
        match event {
            CaptureEvent::Frame(frame) if frame.format == PixelFormat::Bgra8 => {
                self.mailbox.submit(frame);
            }
            CaptureEvent::Frame(_) => self.sink.on_event(CaptureEvent::Suspended(
                "OpenH264 received a non-BGRA source frame".into(),
            )),
            CaptureEvent::GeometryChanged(mut geometry) => {
                geometry.width_px = even_extent(geometry.width_px);
                geometry.height_px = even_extent(geometry.height_px);
                self.sink.on_event(CaptureEvent::GeometryChanged(geometry));
            }
            other => self.sink.on_event(other),
        }
    }
}

impl Drop for WindowsH264Encoder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn encoder_loop(
    max_fps: u16,
    target_bitrate_kbps: Option<u32>,
    sink: Arc<dyn CaptureSink>,
    mailbox: Arc<EncoderMailbox>,
) {
    let mut native: Option<SoftwareEncoder> = None;
    let mut codec_epoch = 0u64;
    while let Some((frame, mut force_keyframe)) = mailbox.take() {
        let dimensions = (even_extent(frame.width_px), even_extent(frame.height_px));
        if native.as_ref().map(SoftwareEncoder::dimensions) != Some(dimensions) {
            codec_epoch = codec_epoch.saturating_add(1);
            match SoftwareEncoder::new(dimensions.0, dimensions.1, max_fps, target_bitrate_kbps) {
                Ok(encoder) => native = Some(encoder),
                Err(error) => {
                    native = None;
                    sink.on_event(CaptureEvent::Suspended(format!(
                        "OpenH264 encoder creation failed: {error}"
                    )));
                    continue;
                }
            }
            force_keyframe = true;
        }
        let Some(encoder) = native.as_mut() else {
            continue;
        };
        match encoder.encode(&frame, force_keyframe) {
            Ok(Some((bytes, keyframe))) => sink.on_event(CaptureEvent::Frame(OwnedFrame {
                bytes: bytes.into(),
                format: PixelFormat::H264AnnexB,
                width_px: dimensions.0,
                height_px: dimensions.1,
                bytes_per_row: None,
                capture_timestamp_us: frame.capture_timestamp_us,
                encode_duration_us: None,
                codec_epoch,
                keyframe,
            })),
            Ok(None) => {}
            Err(error) => {
                sink.on_event(CaptureEvent::Suspended(format!(
                    "OpenH264 frame submission failed: {error}"
                )));
                native = None;
            }
        }
    }
}

struct SoftwareEncoder {
    encoder: Encoder,
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    yuv: YUVBuffer,
    parameter_sets: Vec<u8>,
    initialized: bool,
}

impl SoftwareEncoder {
    fn new(
        width: u32,
        height: u32,
        max_fps: u16,
        target_bitrate_kbps: Option<u32>,
    ) -> Result<Self, String> {
        let bitrate = target_bitrate_kbps
            .map(|kilobits| kilobits.saturating_mul(1_000))
            .unwrap_or_else(|| bitrate_for(width, height, max_fps));
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(f32::from(max_fps.clamp(1, 60))))
            .rate_control_mode(RateControlMode::Bitrate)
            .usage_type(UsageType::ScreenContentRealTime)
            .profile(Profile::Baseline)
            .adaptive_quantization(false)
            .background_detection(false)
            .skip_frames(true)
            .intra_frame_period(IntraFramePeriod::from_num_frames(u32::from(
                max_fps.clamp(1, 60).saturating_mul(2),
            )))
            .vui(VuiConfig::srgb());
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|error| error.to_string())?;
        let pixels = usize::try_from(width)
            .ok()
            .and_then(|width| usize::try_from(height).ok().map(|height| width * height))
            .ok_or_else(|| "OpenH264 dimensions overflow".to_owned())?;
        Ok(Self {
            encoder,
            width,
            height,
            rgb: vec![0; pixels * 3],
            yuv: YUVBuffer::new(width as usize, height as usize),
            parameter_sets: Vec::new(),
            initialized: false,
        })
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn encode(
        &mut self,
        frame: &OwnedFrame,
        force_keyframe: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        let stride = frame
            .bytes_per_row
            .unwrap_or(frame.width_px.saturating_mul(4));
        bgra_to_rgb(
            &frame.bytes,
            stride,
            frame.width_px,
            self.width,
            self.height,
            &mut self.rgb,
        )?;
        self.yuv.read_rgb8(RgbSliceU8::new(
            &self.rgb,
            (self.width as usize, self.height as usize),
        ));
        if force_keyframe && self.initialized {
            self.encoder.force_intra_frame();
        }
        let bitstream = self
            .encoder
            .encode(&self.yuv)
            .map_err(|error| error.to_string())?;
        self.initialized = true;
        if bitstream.frame_type() == FrameType::Skip {
            return Ok(None);
        }
        let mut bytes = bitstream.to_vec();
        if bytes.is_empty() {
            return Ok(None);
        }
        let nal_types = annex_b_nal_types(&bytes);
        let keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I)
            || nal_types.contains(&5);
        let has_sps = nal_types.contains(&7);
        let has_pps = nal_types.contains(&8);
        if has_sps || has_pps {
            self.parameter_sets = parameter_sets(&bytes);
        }
        if keyframe && (!has_sps || !has_pps) {
            if self.parameter_sets.is_empty() {
                return Err("OpenH264 emitted an IDR without SPS/PPS".into());
            }
            let mut prefixed = self.parameter_sets.clone();
            prefixed.extend_from_slice(&bytes);
            bytes = prefixed;
        }
        Ok(Some((bytes, keyframe)))
    }
}

fn even_extent(value: u32) -> u32 {
    value.max(2) & !1
}

fn bitrate_for(width: u32, height: u32, fps: u16) -> u32 {
    ((u64::from(width) * u64::from(height) * u64::from(fps.clamp(1, 60)) * 12) / 100)
        .clamp(250_000, 8_000_000) as u32
}

fn bgra_to_rgb(
    source: &[u8],
    source_stride: u32,
    source_width: u32,
    width: u32,
    height: u32,
    target: &mut [u8],
) -> Result<(), String> {
    let stride = source_stride as usize;
    let width = width as usize;
    let height = height as usize;
    let required_source = stride
        .checked_mul(height)
        .ok_or_else(|| "BGRA source size overflow".to_owned())?;
    let required_target = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| "RGB target size overflow".to_owned())?;
    if source_width < width as u32
        || source.len() < required_source
        || target.len() != required_target
        || stride < width.saturating_mul(4)
    {
        return Err("BGRA source does not cover the encoded dimensions".into());
    }
    for row in 0..height {
        let source_row = &source[row * stride..row * stride + width * 4];
        let target_row = &mut target[row * width * 3..(row + 1) * width * 3];
        for (bgra, rgb) in source_row
            .as_chunks::<4>()
            .0
            .iter()
            .zip(target_row.as_chunks_mut::<3>().0.iter_mut())
        {
            rgb[0] = bgra[2];
            rgb[1] = bgra[1];
            rgb[2] = bgra[0];
        }
    }
    Ok(())
}

fn annex_b_units(bytes: &[u8]) -> Vec<(usize, usize, u8)> {
    let mut starts = Vec::new();
    let mut offset = 0usize;
    while offset + 3 < bytes.len() {
        let start_code = if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
            Some(4)
        } else if bytes[offset..].starts_with(&[0, 0, 1]) {
            Some(3)
        } else {
            None
        };
        if let Some(length) = start_code {
            let header = offset + length;
            if header < bytes.len() {
                starts.push((offset, header, bytes[header] & 0x1f));
            }
            offset = header.saturating_add(1);
        } else {
            offset += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, (start, _, kind))| {
            let end = starts.get(index + 1).map_or(bytes.len(), |next| next.0);
            (*start, end, *kind)
        })
        .collect()
}

fn annex_b_nal_types(bytes: &[u8]) -> Vec<u8> {
    annex_b_units(bytes)
        .into_iter()
        .map(|(_, _, kind)| kind)
        .collect()
}

fn parameter_sets(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for (start, end, kind) in annex_b_units(bytes) {
        if matches!(kind, 7 | 8) {
            output.extend_from_slice(&bytes[start..end]);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(timestamp: u64) -> OwnedFrame {
        OwnedFrame {
            bytes: vec![0x80; 64 * 64 * 4].into(),
            format: PixelFormat::Bgra8,
            width_px: 64,
            height_px: 64,
            bytes_per_row: Some(64 * 4),
            capture_timestamp_us: timestamp,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        }
    }

    #[test]
    fn keyframe_request_replays_the_last_static_frame() {
        let mailbox = EncoderMailbox::new();
        mailbox.submit(frame(7));
        let (initial, force) = mailbox.take().unwrap();
        assert_eq!(initial.capture_timestamp_us, 7);
        assert!(!force);

        mailbox.request_keyframe();
        let (replayed, force) = mailbox.take().unwrap();
        assert_eq!(replayed.capture_timestamp_us, 7);
        assert!(force);
    }

    #[test]
    fn software_encoder_outputs_independently_decodable_idr() {
        let mut encoder = SoftwareEncoder::new(64, 64, 30, None).unwrap();
        let frame = frame(1);
        let (bytes, keyframe) = encoder.encode(&frame, true).unwrap().unwrap();
        let types = annex_b_nal_types(&bytes);
        assert!(keyframe);
        assert!(types.contains(&7));
        assert!(types.contains(&8));
        assert!(types.contains(&5));
    }

    #[test]
    fn odd_capture_dimensions_crop_to_even_encoder_extents() {
        assert_eq!(even_extent(801), 800);
        assert_eq!(even_extent(1), 2);
    }
}
