// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Apple VideoToolbox H.264 / HEVC encoder and decoder.
//!
//! The encoder logic (real-time session, no reordering, Annex B output with
//! parameter sets prepended to every IDR) is adapted from
//! `cua-spacesd-desktop/src/macos_h264.rs` in this repository (Apache-2.0). This
//! copy is synchronous: every `encode` completes its frame before returning,
//! which keeps the one-in/one-out contract of [`VideoEncoder`].

pub mod ffi;

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::time::Instant;

use ffi::*;

use crate::convert;
use crate::error::{CodecError, Result};
use crate::types::{
    AccessUnit, Backend, DecodedFrame, EncoderConfig, FrameData, KeyframePolicy, Preset,
    RateControl, Reconfigured, VideoCodec, VideoFrame,
};
use crate::video::{EpochState, VideoDecoder, VideoEncoder};

const BACKEND: Backend = Backend::VideoToolbox;

fn codec_type(codec: VideoCodec) -> Result<u32> {
    match codec {
        VideoCodec::H264 => Ok(kCMVideoCodecType_H264),
        VideoCodec::Hevc => Ok(kCMVideoCodecType_HEVC),
        VideoCodec::Av1 => Err(CodecError::Unsupported(
            "VideoToolbox cannot encode AV1".into(),
        )),
    }
}

struct PendingFrame {
    pts_us: u64,
    started: Instant,
}

#[derive(Default)]
struct EncodeOutput {
    units: Vec<(u64, Instant, Vec<u8>, bool)>,
    error: Option<String>,
}

struct EncodeContext {
    codec: VideoCodec,
    output: Mutex<EncodeOutput>,
}

/// A VideoToolbox compression session.
pub struct VtEncoder {
    config: EncoderConfig,
    session: NonNull<c_void>,
    context: Box<EncodeContext>,
    epoch: EpochState,
    hardware: bool,
}

// SAFETY: the session is only used through &mut self; VideoToolbox sessions
// may be driven from any thread.
unsafe impl Send for VtEncoder {}

impl VtEncoder {
    /// Opens a session on the hardware encoder when the Mac has one.
    ///
    /// H.264 first asks for the low-latency rate controller (macOS 11.3+).
    /// On some Macs that session is the software encoder
    /// (`UsingHardwareAcceleratedVideoEncoder` is false), so a software
    /// low-latency session is retried without it and the hardware one is kept.
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        config.validate()?;
        if config.codec != VideoCodec::H264 {
            return Self::create(config, false);
        }
        match Self::create(config, true) {
            Ok(enc) if enc.hardware => Ok(enc),
            Ok(enc) => match Self::create(config, false) {
                Ok(plain) if plain.hardware => Ok(plain),
                _ => Ok(enc),
            },
            Err(_) => Self::create(config, false),
        }
    }

    fn create(config: &EncoderConfig, low_latency_rc: bool) -> Result<Self> {
        let codec = codec_type(config.codec)?;
        let context = Box::new(EncodeContext {
            codec: config.codec,
            output: Mutex::new(EncodeOutput::default()),
        });
        let source_attrs = {
            let fmt = cf_i32(kCVPixelFormatType_32BGRA as i32);
            let w = cf_i32(config.width as i32);
            let h = cf_i32(config.height as i32);
            let io = cf_dict(&[]);
            unsafe {
                cf_dict(&[
                    (kCVPixelBufferPixelFormatTypeKey, fmt.as_ptr()),
                    (kCVPixelBufferWidthKey, w.as_ptr()),
                    (kCVPixelBufferHeightKey, h.as_ptr()),
                    (kCVPixelBufferIOSurfacePropertiesKey, io.as_ptr()),
                ])
            }
        };
        let low_latency = unsafe {
            cf_dict(&[(
                kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
                cf_bool(true),
            )])
        };
        let mut session = std::ptr::null_mut();
        let ctx_ptr = (&*context as *const EncodeContext)
            .cast_mut()
            .cast::<c_void>();
        let spec = low_latency_rc.then_some(&low_latency);
        let status = unsafe {
            VTCompressionSessionCreate(
                std::ptr::null(),
                config.width as i32,
                config.height as i32,
                codec,
                spec.map_or(std::ptr::null(), |s| s.as_ptr()),
                source_attrs.as_ptr(),
                std::ptr::null(),
                Some(compression_output),
                ctx_ptr,
                &mut session,
            )
        };
        let session = NonNull::new(session)
            .filter(|_| status == 0)
            .ok_or_else(|| {
                CodecError::backend(
                    BACKEND,
                    format!("VTCompressionSessionCreate returned {status}"),
                )
            })?;
        let mut encoder = Self {
            config: config.clone(),
            session,
            context,
            epoch: EpochState::new(),
            hardware: false,
        };
        encoder.apply_properties(true)?;
        encoder.hardware = encoder.query_hardware();
        Ok(encoder)
    }

    /// Whether VideoToolbox picked a hardware encoder for this session.
    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    fn query_hardware(&self) -> bool {
        let mut value: CFTypeRef = std::ptr::null();
        let status = unsafe {
            VTSessionCopyProperty(
                self.session.as_ptr(),
                kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                std::ptr::null(),
                &mut value,
            )
        };
        if status != 0 || value.is_null() {
            return false;
        }
        let hw = value == unsafe { kCFBooleanTrue };
        unsafe { CFRelease(value) };
        hw
    }

    fn set(&self, key: CFTypeRef, value: CFTypeRef) -> Result<()> {
        let status = unsafe { VTSessionSetProperty(self.session.as_ptr(), key, value) };
        if status == 0 {
            Ok(())
        } else {
            Err(CodecError::backend(
                BACKEND,
                format!("VTSessionSetProperty returned {status}"),
            ))
        }
    }

    fn set_optional(&self, key: CFTypeRef, value: CFTypeRef) {
        if let Err(e) = self.set(key, value) {
            tracing::debug!("optional VideoToolbox property rejected: {e}");
        }
    }

    fn apply_rate(&self) -> Result<()> {
        let cfg = &self.config;
        self.set(
            unsafe { kVTCompressionPropertyKey_ExpectedFrameRate },
            cf_i32(cfg.fps as i32).as_ptr(),
        )?;
        let bps = i32::try_from(u64::from(cfg.bitrate_kbps) * 1000).unwrap_or(i32::MAX);
        self.set(
            unsafe { kVTCompressionPropertyKey_AverageBitRate },
            cf_i32(bps).as_ptr(),
        )?;
        match cfg.rate_control {
            RateControl::Cbr | RateControl::Vbr => {
                // Cap bytes per second (1.1x for CBR, 1.5x for VBR) over 1 s.
                let factor = if cfg.rate_control == RateControl::Cbr {
                    1.1
                } else {
                    1.5
                };
                let bytes = cf_i32(((f64::from(bps) / 8.0) * factor) as i32);
                let secs = cf_f32(1.0);
                let limits = cf_array(&[bytes.as_ptr(), secs.as_ptr()]);
                self.set_optional(
                    unsafe { kVTCompressionPropertyKey_DataRateLimits },
                    limits.as_ptr(),
                );
            }
            RateControl::ConstantQuality { quality } => {
                self.set_optional(
                    unsafe { kVTCompressionPropertyKey_Quality },
                    cf_f32(f32::from(quality.min(100)) / 100.0).as_ptr(),
                );
            }
        }
        Ok(())
    }

    fn apply_properties(&mut self, prepare: bool) -> Result<()> {
        let cfg = self.config.clone();
        unsafe {
            self.set(kVTCompressionPropertyKey_RealTime, cf_bool(true))?;
            self.set(
                kVTCompressionPropertyKey_AllowFrameReordering,
                cf_bool(false),
            )?;
            self.set_optional(
                kVTCompressionPropertyKey_MaxFrameDelayCount,
                cf_i32(0).as_ptr(),
            );
            let interval = match cfg.keyframe_policy {
                KeyframePolicy::Periodic { interval_frames } => interval_frames as i32,
                // 0 = unlimited GOP; IDRs only when forced.
                KeyframePolicy::IdrOnDemand | KeyframePolicy::IntraRefresh { .. } => 0,
            };
            self.set_optional(
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                cf_i32(interval).as_ptr(),
            );
            let profile = match (cfg.codec, cfg.preset) {
                (VideoCodec::Hevc, _) => kVTProfileLevel_HEVC_Main_AutoLevel,
                (_, Preset::Quality) => kVTProfileLevel_H264_ConstrainedHigh_AutoLevel,
                _ => kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
            };
            self.set_optional(kVTCompressionPropertyKey_ProfileLevel, profile);
            if cfg.preset == Preset::UltraLowLatency {
                self.set_optional(
                    kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
                    cf_bool(true),
                );
            }
            self.set_optional(
                kVTCompressionPropertyKey_ColorPrimaries,
                kCVImageBufferColorPrimaries_ITU_R_709_2,
            );
            self.set_optional(
                kVTCompressionPropertyKey_TransferFunction,
                kCVImageBufferTransferFunction_ITU_R_709_2,
            );
            self.set_optional(
                kVTCompressionPropertyKey_YCbCrMatrix,
                kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            );
        }
        self.apply_rate()?;
        if prepare {
            let status =
                unsafe { VTCompressionSessionPrepareToEncodeFrames(self.session.as_ptr()) };
            if status != 0 {
                return Err(CodecError::backend(
                    BACKEND,
                    format!("PrepareToEncodeFrames returned {status}"),
                ));
            }
        }
        Ok(())
    }

    fn restart(&mut self, config: &EncoderConfig) -> Result<u64> {
        let fresh = Self::new(config)?;
        let epoch = self.epoch.clone();
        *self = fresh;
        self.epoch = epoch;
        Ok(self.epoch.restart())
    }

    fn pixel_buffer_for(&self, frame: &VideoFrame<'_>) -> Result<CfOwned> {
        let (w, h) = (frame.width as usize, frame.height as usize);
        match frame.data {
            FrameData::CvPixelBuffer(buffer) => {
                unsafe { CFRetain(buffer.as_ptr()) };
                Ok(CfOwned(buffer.as_ptr()))
            }
            FrameData::Bgra { data, stride } => {
                let pool = unsafe { VTCompressionSessionGetPixelBufferPool(self.session.as_ptr()) };
                let mut pb = std::ptr::null_mut();
                let status = if pool.is_null() {
                    unsafe {
                        CVPixelBufferCreate(
                            std::ptr::null(),
                            w,
                            h,
                            kCVPixelFormatType_32BGRA,
                            std::ptr::null(),
                            &mut pb,
                        )
                    }
                } else {
                    unsafe { CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), pool, &mut pb) }
                };
                if status != 0 || pb.is_null() {
                    return Err(CodecError::backend(
                        BACKEND,
                        format!("pixel buffer allocation returned {status}"),
                    ));
                }
                let owned = CfOwned(pb);
                if data.len() < stride * (h - 1) + w * 4 {
                    return Err(CodecError::InvalidArgument(
                        "BGRA buffer smaller than its geometry".into(),
                    ));
                }
                unsafe {
                    CVPixelBufferLockBaseAddress(pb, 0);
                    let base = CVPixelBufferGetBaseAddress(pb).cast::<u8>();
                    let dst_stride = CVPixelBufferGetBytesPerRow(pb);
                    for row in 0..h {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr().add(row * stride),
                            base.add(row * dst_stride),
                            w * 4,
                        );
                    }
                    CVPixelBufferUnlockBaseAddress(pb, 0);
                }
                Ok(owned)
            }
            FrameData::Nv12 { .. } | FrameData::I420 { .. } => {
                let (y, uv, y_stride, uv_stride);
                let tmp;
                match frame.data {
                    FrameData::Nv12 {
                        y: yy,
                        y_stride: ys,
                        uv: uvp,
                        uv_stride: uvs,
                    } => {
                        y = yy;
                        uv = uvp;
                        y_stride = ys;
                        uv_stride = uvs;
                    }
                    FrameData::I420 {
                        y: yy,
                        u,
                        v,
                        strides,
                    } => {
                        let compact =
                            convert::i420_compact(frame.width, frame.height, yy, u, v, strides);
                        tmp = convert::i420_to_nv12(&compact);
                        y = &tmp.0;
                        uv = &tmp.1;
                        y_stride = w;
                        uv_stride = w.div_ceil(2) * 2;
                    }
                    _ => unreachable!(),
                }
                let mut pb = std::ptr::null_mut();
                let status = unsafe {
                    CVPixelBufferCreate(
                        std::ptr::null(),
                        w,
                        h,
                        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                        std::ptr::null(),
                        &mut pb,
                    )
                };
                if status != 0 || pb.is_null() {
                    return Err(CodecError::backend(
                        BACKEND,
                        format!("CVPixelBufferCreate(420v) returned {status}"),
                    ));
                }
                let owned = CfOwned(pb);
                unsafe {
                    CVPixelBufferLockBaseAddress(pb, 0);
                    for (plane, src, src_stride, rows, row_bytes) in [
                        (0usize, y, y_stride, h, w),
                        (1usize, uv, uv_stride, h.div_ceil(2), w.div_ceil(2) * 2),
                    ] {
                        let base = CVPixelBufferGetBaseAddressOfPlane(pb, plane).cast::<u8>();
                        let dst_stride = CVPixelBufferGetBytesPerRowOfPlane(pb, plane);
                        for row in 0..rows {
                            std::ptr::copy_nonoverlapping(
                                src.as_ptr().add(row * src_stride),
                                base.add(row * dst_stride),
                                row_bytes,
                            );
                        }
                    }
                    CVPixelBufferUnlockBaseAddress(pb, 0);
                }
                Ok(owned)
            }
            FrameData::DmaBuf(_) => Err(CodecError::Unsupported(
                "DMA-BUF frames are Linux-only".into(),
            )),
        }
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = VTCompressionSessionCompleteFrames(self.session.as_ptr(), CMTime::INVALID);
            VTCompressionSessionInvalidate(self.session.as_ptr());
            CFRelease(self.session.as_ptr());
        }
    }
}

impl VideoEncoder for VtEncoder {
    fn backend(&self) -> Backend {
        BACKEND
    }

    fn config(&self) -> &EncoderConfig {
        &self.config
    }

    fn codec_epoch(&self) -> u64 {
        self.epoch.epoch
    }

    fn reconfigure(&mut self, config: &EncoderConfig) -> Result<Reconfigured> {
        config.validate()?;
        if config.needs_restart(&self.config) {
            let epoch = self.restart(config)?;
            return Ok(Reconfigured::Restarted { codec_epoch: epoch });
        }
        let previous = std::mem::replace(&mut self.config, config.clone());
        if let Err(e) = self.apply_rate() {
            self.config = previous;
            return Err(e);
        }
        Ok(Reconfigured::InPlace)
    }

    fn force_keyframe(&mut self) {
        self.epoch.force_keyframe = true;
    }

    fn encode(&mut self, frame: &VideoFrame<'_>) -> Result<Vec<AccessUnit>> {
        if frame.width != self.config.width || frame.height != self.config.height {
            let mut next = self.config.clone();
            next.width = frame.width;
            next.height = frame.height;
            self.restart(&next)?;
        }
        let pixel_buffer = self.pixel_buffer_for(frame)?;
        let force = std::mem::take(&mut self.epoch.force_keyframe);
        let props = force.then(|| unsafe {
            cf_dict(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, kCFBooleanTrue)])
        });
        let pending = Box::into_raw(Box::new(PendingFrame {
            pts_us: frame.pts_us,
            started: Instant::now(),
        }));
        let pts = CMTime::new(i64::try_from(frame.pts_us).unwrap_or(i64::MAX), 1_000_000);
        let duration = CMTime::new(1, self.config.fps as i32);
        let mut info = 0u32;
        let status = unsafe {
            VTCompressionSessionEncodeFrame(
                self.session.as_ptr(),
                pixel_buffer.as_ptr().cast_mut(),
                pts,
                duration,
                props.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                pending.cast(),
                &mut info,
            )
        };
        if status != 0 {
            unsafe { drop(Box::from_raw(pending)) };
            self.epoch.force_keyframe |= force;
            return Err(CodecError::backend(
                BACKEND,
                format!("VTCompressionSessionEncodeFrame returned {status}"),
            ));
        }
        let status = unsafe { VTCompressionSessionCompleteFrames(self.session.as_ptr(), pts) };
        if status != 0 {
            return Err(CodecError::backend(
                BACKEND,
                format!("VTCompressionSessionCompleteFrames returned {status}"),
            ));
        }
        let mut out = self
            .context
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = out.error.take() {
            return Err(CodecError::backend(BACKEND, error));
        }
        let units = std::mem::take(&mut out.units);
        drop(out);
        Ok(units
            .into_iter()
            .map(|(pts_us, started, data, keyframe)| AccessUnit {
                data,
                codec: self.config.codec,
                keyframe,
                codec_epoch: self.epoch.epoch,
                pts_us,
                width: self.config.width,
                height: self.config.height,
                encode_duration_us: u32::try_from(started.elapsed().as_micros())
                    .unwrap_or(u32::MAX),
                backend: BACKEND,
            })
            .collect())
    }
}

unsafe extern "C" fn compression_output(
    output_refcon: *mut c_void,
    source_refcon: *mut c_void,
    status: i32,
    info_flags: u32,
    sample_buffer: *mut c_void,
) {
    let pending = (!source_refcon.is_null())
        .then(|| unsafe { Box::from_raw(source_refcon.cast::<PendingFrame>()) });
    if output_refcon.is_null() {
        return;
    }
    let context = unsafe { &*output_refcon.cast::<EncodeContext>() };
    let mut out = context
        .output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if status != 0 {
        out.error = Some(format!("VideoToolbox output callback status {status}"));
        return;
    }
    if sample_buffer.is_null() || info_flags & kVTEncodeInfo_FrameDropped != 0 {
        return;
    }
    let Some(pending) = pending else { return };
    match unsafe { sample_to_annex_b(context.codec, sample_buffer) } {
        Some((data, keyframe)) => out
            .units
            .push((pending.pts_us, pending.started, data, keyframe)),
        None => out.error = Some("VideoToolbox produced an unparseable sample".into()),
    }
}

/// Converts an encoded CMSampleBuffer (length-prefixed NALs) to Annex B,
/// prepending the parameter sets to keyframes.
unsafe fn sample_to_annex_b(codec: VideoCodec, sample: *mut c_void) -> Option<(Vec<u8>, bool)> {
    let format = unsafe { CMSampleBufferGetFormatDescription(sample) };
    let block = unsafe { CMSampleBufferGetDataBuffer(sample) };
    if format.is_null() || block.is_null() {
        return None;
    }
    let get = match codec {
        VideoCodec::H264 => CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
        VideoCodec::Hevc => CMVideoFormatDescriptionGetHEVCParameterSetAtIndex,
        VideoCodec::Av1 => return None,
    };
    let mut count = 0usize;
    let mut nal_len = 0i32;
    let status = unsafe {
        get(
            format,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut count,
            &mut nal_len,
        )
    };
    if status != 0 || !(1..=4).contains(&nal_len) {
        return None;
    }
    let len = unsafe { CMBlockBufferGetDataLength(block) };
    let mut raw = vec![0u8; len];
    if unsafe { CMBlockBufferCopyDataBytes(block, 0, len, raw.as_mut_ptr().cast()) } != 0 {
        return None;
    }
    let (body, keyframe) =
        crate::bitstream::length_prefixed_to_annex_b(codec, &raw, nal_len as usize)?;
    if !keyframe {
        return Some((body, false));
    }
    let mut out = Vec::with_capacity(body.len() + 128);
    for index in 0..count {
        let mut ptr = std::ptr::null();
        let mut size = 0usize;
        if unsafe {
            get(
                format,
                index,
                &mut ptr,
                &mut size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } != 0
            || ptr.is_null()
        {
            return None;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr, size) });
    }
    out.extend_from_slice(&body);
    Some((out, true))
}

// ---------------------------------------------------------------- decoder

#[derive(Default)]
struct DecodeOutput {
    frame: Option<DecodedFrame>,
    error: Option<i32>,
}

/// A VideoToolbox decompression session (H.264 / HEVC Annex B input).
pub struct VtDecoder {
    codec: VideoCodec,
    format: Option<CfOwned>,
    session: Option<NonNull<c_void>>,
    parameter_sets: Vec<Vec<u8>>,
    output: Box<Mutex<DecodeOutput>>,
}

// SAFETY: used through &mut self only.
unsafe impl Send for VtDecoder {}

impl VtDecoder {
    /// Creates a decoder for `codec` (H.264 or HEVC).
    pub fn new(codec: VideoCodec) -> Result<Self> {
        codec_type(codec)?;
        Ok(Self {
            codec,
            format: None,
            session: None,
            parameter_sets: Vec::new(),
            output: Box::new(Mutex::new(DecodeOutput::default())),
        })
    }

    fn is_parameter_set(&self, nal: &[u8]) -> bool {
        matches!(
            (self.codec, crate::bitstream::nal_type(self.codec, nal)),
            (VideoCodec::H264, Some(7 | 8)) | (VideoCodec::Hevc, Some(32..=34))
        )
    }

    fn teardown(&mut self) {
        if let Some(session) = self.session.take() {
            unsafe {
                VTDecompressionSessionInvalidate(session.as_ptr());
                CFRelease(session.as_ptr());
            }
        }
        self.format = None;
    }

    fn open(&mut self, sets: Vec<Vec<u8>>) -> Result<()> {
        self.teardown();
        let ptrs: Vec<*const u8> = sets.iter().map(|s| s.as_ptr()).collect();
        let sizes: Vec<usize> = sets.iter().map(Vec::len).collect();
        let mut format = std::ptr::null_mut();
        let status = unsafe {
            match self.codec {
                VideoCodec::H264 => CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    std::ptr::null(),
                    sets.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut format,
                ),
                _ => CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    std::ptr::null(),
                    sets.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    std::ptr::null(),
                    &mut format,
                ),
            }
        };
        if status != 0 || format.is_null() {
            return Err(CodecError::backend(
                BACKEND,
                format!("format description from parameter sets returned {status}"),
            ));
        }
        let format = CfOwned(format);
        let pix = cf_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
        let attrs = unsafe { cf_dict(&[(kCVPixelBufferPixelFormatTypeKey, pix.as_ptr())]) };
        let record = VTDecompressionOutputCallbackRecord {
            callback: Some(decompression_output),
            refcon: (&*self.output as *const Mutex<DecodeOutput>)
                .cast_mut()
                .cast(),
        };
        let mut session = std::ptr::null_mut();
        let status = unsafe {
            VTDecompressionSessionCreate(
                std::ptr::null(),
                format.as_ptr().cast_mut(),
                std::ptr::null(),
                attrs.as_ptr(),
                &record,
                &mut session,
            )
        };
        let session = NonNull::new(session)
            .filter(|_| status == 0)
            .ok_or_else(|| {
                CodecError::backend(
                    BACKEND,
                    format!("VTDecompressionSessionCreate returned {status}"),
                )
            })?;
        self.session = Some(session);
        self.format = Some(format);
        self.parameter_sets = sets;
        Ok(())
    }
}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl VideoDecoder for VtDecoder {
    /// Whether the open session decodes in hardware. `None` until the first
    /// keyframe opens a session.
    fn is_hardware(&self) -> Option<bool> {
        let session = self.session?;
        let mut value: CFTypeRef = std::ptr::null();
        let status = unsafe {
            VTSessionCopyProperty(
                session.as_ptr(),
                kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                std::ptr::null(),
                &mut value,
            )
        };
        if status != 0 || value.is_null() {
            return Some(false);
        }
        let hw = value == unsafe { kCFBooleanTrue };
        unsafe { CFRelease(value) };
        Some(hw)
    }

    fn backend(&self) -> Backend {
        BACKEND
    }

    fn codec(&self) -> VideoCodec {
        self.codec
    }

    fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Option<DecodedFrame>> {
        let nals = crate::bitstream::annex_b_nals(data);
        let sets: Vec<Vec<u8>> = nals
            .iter()
            .filter(|n| self.is_parameter_set(n))
            .map(|n| n.to_vec())
            .collect();
        if !sets.is_empty() && (self.session.is_none() || sets != self.parameter_sets) {
            self.open(sets)?;
        }
        let (Some(session), Some(format)) = (self.session, self.format.as_ref()) else {
            return Ok(None); // Waiting for a keyframe with parameter sets.
        };
        let body = crate::bitstream::annex_b_to_length_prefixed(data, |n| {
            matches!(
                (self.codec, crate::bitstream::nal_type(self.codec, n)),
                (VideoCodec::H264, Some(7..=9)) | (VideoCodec::Hevc, Some(32..=35))
            )
        });
        if body.is_empty() {
            return Ok(None);
        }
        let mut block = std::ptr::null_mut();
        let status = unsafe {
            CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                std::ptr::null_mut(),
                body.len(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                body.len(),
                kCMBlockBufferAssureMemoryNowFlag,
                &mut block,
            )
        };
        if status != 0 || block.is_null() {
            return Err(CodecError::backend(
                BACKEND,
                format!("CMBlockBufferCreate returned {status}"),
            ));
        }
        let block = CfOwned(block);
        unsafe {
            CMBlockBufferReplaceDataBytes(
                body.as_ptr().cast(),
                block.as_ptr().cast_mut(),
                0,
                body.len(),
            )
        };
        let mut sample = std::ptr::null_mut();
        let size = body.len();
        let status = unsafe {
            CMSampleBufferCreateReady(
                std::ptr::null(),
                block.as_ptr().cast_mut(),
                format.as_ptr().cast_mut(),
                1,
                0,
                std::ptr::null(),
                1,
                &size,
                &mut sample,
            )
        };
        if status != 0 || sample.is_null() {
            return Err(CodecError::backend(
                BACKEND,
                format!("CMSampleBufferCreateReady returned {status}"),
            ));
        }
        let sample = CfOwned(sample);
        let mut info = 0u32;
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                session.as_ptr(),
                sample.as_ptr().cast_mut(),
                0,
                std::ptr::null_mut(),
                &mut info,
            )
        };
        unsafe { VTDecompressionSessionWaitForAsynchronousFrames(session.as_ptr()) };
        if status != 0 {
            return Err(CodecError::backend(
                BACKEND,
                format!("VTDecompressionSessionDecodeFrame returned {status}"),
            ));
        }
        let mut out = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(status) = out.error.take() {
            return Err(CodecError::backend(
                BACKEND,
                format!("decode callback status {status}"),
            ));
        }
        Ok(out.frame.take().map(|mut f| {
            f.pts_us = pts_us;
            f
        }))
    }

    fn reset(&mut self) -> Result<()> {
        self.teardown();
        self.parameter_sets.clear();
        Ok(())
    }
}

unsafe extern "C" fn decompression_output(
    output_refcon: *mut c_void,
    _source_refcon: *mut c_void,
    status: i32,
    _info_flags: u32,
    image: *mut c_void,
    _pts: CMTime,
    _duration: CMTime,
) {
    if output_refcon.is_null() {
        return;
    }
    let output = unsafe { &*output_refcon.cast::<Mutex<DecodeOutput>>() };
    let mut out = output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if status != 0 {
        out.error = Some(status);
        return;
    }
    if image.is_null() {
        return;
    }
    unsafe {
        CVPixelBufferLockBaseAddress(image, kCVPixelBufferLock_ReadOnly);
        let w = CVPixelBufferGetWidth(image);
        let h = CVPixelBufferGetHeight(image);
        let y_ptr = CVPixelBufferGetBaseAddressOfPlane(image, 0).cast::<u8>();
        let uv_ptr = CVPixelBufferGetBaseAddressOfPlane(image, 1).cast::<u8>();
        let ys = CVPixelBufferGetBytesPerRowOfPlane(image, 0);
        let uvs = CVPixelBufferGetBytesPerRowOfPlane(image, 1);
        if !y_ptr.is_null() && !uv_ptr.is_null() {
            let y = std::slice::from_raw_parts(y_ptr, ys * h);
            let uv = std::slice::from_raw_parts(uv_ptr, uvs * h.div_ceil(2));
            let buf = convert::nv12_to_i420(w as u32, h as u32, y, ys, uv, uvs);
            out.frame = Some(DecodedFrame {
                width: buf.width,
                height: buf.height,
                y: buf.y,
                u: buf.u,
                v: buf.v,
                pts_us: 0,
            });
        }
        CVPixelBufferUnlockBaseAddress(image, kCVPixelBufferLock_ReadOnly);
    }
}

/// Real test encode used by the probe. Returns whether the session used the
/// hardware encoder.
pub fn probe_encode(codec: VideoCodec) -> Result<bool> {
    let cfg = EncoderConfig::new(codec, 320, 240, 30).with_bitrate_kbps(500);
    let mut enc = VtEncoder::new(&cfg)?;
    let pixels = vec![0x80u8; 320 * 240 * 4];
    let aus = enc.encode(&VideoFrame::bgra(320, 240, 0, &pixels))?;
    let au = aus
        .first()
        .ok_or_else(|| CodecError::backend(BACKEND, "probe encode produced no output"))?;
    if !au.keyframe || !crate::bitstream::has_parameter_sets(codec, &au.data) {
        return Err(CodecError::backend(
            BACKEND,
            "probe encode did not start with IDR + parameter sets",
        ));
    }
    Ok(enc.is_hardware())
}

/// True if VideoToolbox has a hardware decoder for `codec`.
pub fn hardware_decode_supported(codec: VideoCodec) -> bool {
    let ty = match codec {
        VideoCodec::H264 => kCMVideoCodecType_H264,
        VideoCodec::Hevc => kCMVideoCodecType_HEVC,
        VideoCodec::Av1 => u32::from_be_bytes(*b"av01"),
    };
    unsafe { VTIsHardwareDecodeSupported(ty) != 0 }
}
