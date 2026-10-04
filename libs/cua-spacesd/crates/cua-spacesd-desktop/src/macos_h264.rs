// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Low-latency H.264 encoding through macOS VideoToolbox.
//!
//! ScreenCaptureKit frames enter through a one-slot replaceable mailbox so
//! encoder work never blocks its capture callback. Output is Annex B, and
//! every IDR is prefixed with SPS/PPS so each codec epoch is independently
//! decodable.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use cua_spacesd_provider_api::{
    CaptureEvent, CaptureSink, OwnedFrame, PixelFormat, ProviderError, ProviderErrorCode,
};
use screencapturekit::cm::CMSampleBuffer;
use screencapturekit::cv::CVPixelBuffer;

const PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const CODEC_H264: u32 = u32::from_be_bytes(*b"avc1");
const CM_TIME_VALID: u32 = 1;
const CF_NUMBER_SINT32: isize = 3;

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct CMTimeRaw {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTimeRaw {
    const INVALID: Self = Self {
        value: 0,
        timescale: 0,
        flags: 0,
        epoch: 0,
    };

    const fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: CM_TIME_VALID,
            epoch: 0,
        }
    }
}

type OutputCallback = unsafe extern "C" fn(
    output_refcon: *mut c_void,
    source_refcon: *mut c_void,
    status: i32,
    info_flags: u32,
    sample_buffer: *mut c_void,
);

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTCompressionSessionCreate(
        allocator: *const c_void,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: *const c_void,
        source_image_buffer_attributes: *const c_void,
        compressed_data_allocator: *const c_void,
        output_callback: Option<OutputCallback>,
        output_callback_refcon: *mut c_void,
        compression_session_out: *mut *mut c_void,
    ) -> i32;
    fn VTCompressionSessionPrepareToEncodeFrames(session: *mut c_void) -> i32;
    fn VTCompressionSessionEncodeFrame(
        session: *mut c_void,
        image_buffer: *mut c_void,
        presentation_timestamp: CMTimeRaw,
        duration: CMTimeRaw,
        frame_properties: *const c_void,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> i32;
    fn VTCompressionSessionCompleteFrames(
        session: *mut c_void,
        complete_until_presentation_timestamp: CMTimeRaw,
    ) -> i32;
    fn VTCompressionSessionInvalidate(session: *mut c_void);
    fn VTSessionSetProperty(
        session: *mut c_void,
        property_key: *const c_void,
        property_value: *const c_void,
    ) -> i32;

    static kVTCompressionPropertyKey_RealTime: *const c_void;
    static kVTCompressionPropertyKey_AllowFrameReordering: *const c_void;
    static kVTCompressionPropertyKey_ExpectedFrameRate: *const c_void;
    static kVTCompressionPropertyKey_AverageBitRate: *const c_void;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: *const c_void;
    static kVTCompressionPropertyKey_ProfileLevel: *const c_void;
    static kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel: *const c_void;
    static kVTEncodeFrameOptionKey_ForceKeyFrame: *const c_void;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        video_description: *mut c_void,
        parameter_set_index: usize,
        parameter_set_pointer_out: *mut *const u8,
        parameter_set_size_out: *mut usize,
        parameter_set_count_out: *mut usize,
        nal_unit_header_length_out: *mut i32,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFNumberCreate(
        allocator: *const c_void,
        number_type: isize,
        value_ptr: *const c_void,
    ) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    fn CFRelease(value: *const c_void);
    static kCFBooleanTrue: *const c_void;
    static kCFBooleanFalse: *const c_void;
}

#[derive(Debug, Clone)]
enum EncoderInput {
    Owned(OwnedFrame),
    PixelBuffer {
        buffer: CVPixelBuffer,
        width_px: u32,
        height_px: u32,
        capture_timestamp_us: u64,
    },
}

impl EncoderInput {
    const fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Owned(frame) => (frame.width_px, frame.height_px),
            Self::PixelBuffer {
                width_px,
                height_px,
                ..
            } => (*width_px, *height_px),
        }
    }

    const fn capture_timestamp_us(&self) -> u64 {
        match self {
            Self::Owned(frame) => frame.capture_timestamp_us,
            Self::PixelBuffer {
                capture_timestamp_us,
                ..
            } => *capture_timestamp_us,
        }
    }
}

#[derive(Default)]
struct EncoderQueue {
    latest: Option<EncoderInput>,
    last_submitted: Option<EncoderInput>,
    force_keyframe: bool,
    closed: bool,
    replaced_frames: u64,
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

    fn submit(&self, frame: EncoderInput) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.last_submitted = Some(frame.clone());
        if state.latest.replace(frame).is_some() {
            state.replaced_frames = state.replaced_frames.saturating_add(1);
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

    fn take(&self) -> Option<(EncoderInput, bool)> {
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

pub(super) struct MacosH264Encoder {
    sink: Arc<dyn CaptureSink>,
    mailbox: Arc<EncoderMailbox>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl MacosH264Encoder {
    pub(super) fn start(
        max_fps: u16,
        target_bitrate_kbps: Option<u32>,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Arc<Self>, ProviderError> {
        let mailbox = Arc::new(EncoderMailbox::new());
        let worker_mailbox = mailbox.clone();
        let worker_sink = sink.clone();
        let worker = std::thread::Builder::new()
            .name("rcdp-macos-h264".into())
            .spawn(move || encoder_loop(max_fps, target_bitrate_kbps, worker_sink, worker_mailbox))
            .map_err(|error| {
                ProviderError::new(
                    ProviderErrorCode::CaptureFailed,
                    format!("failed to start VideoToolbox worker: {error}"),
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

    pub(super) fn submit_pixel_buffer(
        &self,
        buffer: CVPixelBuffer,
        capture_timestamp_us: u64,
    ) -> Result<(), ProviderError> {
        let width_px = u32::try_from(buffer.width()).map_err(|_| {
            ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                "ScreenCaptureKit frame width exceeds u32",
            )
        })?;
        let height_px = u32::try_from(buffer.height()).map_err(|_| {
            ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                "ScreenCaptureKit frame height exceeds u32",
            )
        })?;
        if width_px == 0 || height_px == 0 || buffer.pixel_format() != PIXEL_FORMAT_BGRA {
            return Err(ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                "ScreenCaptureKit emitted an invalid non-BGRA pixel buffer",
            ));
        }
        self.mailbox.submit(EncoderInput::PixelBuffer {
            buffer,
            width_px,
            height_px,
            capture_timestamp_us,
        });
        Ok(())
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

impl CaptureSink for MacosH264Encoder {
    fn on_event(&self, event: CaptureEvent) {
        match event {
            CaptureEvent::Frame(frame) if frame.format == PixelFormat::Bgra8 => {
                self.mailbox.submit(EncoderInput::Owned(frame));
            }
            CaptureEvent::Frame(_) => self.sink.on_event(CaptureEvent::Suspended(
                "VideoToolbox received a non-BGRA source frame".into(),
            )),
            other => self.sink.on_event(other),
        }
    }
}

impl Drop for MacosH264Encoder {
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
    let mut native: Option<NativeEncoder> = None;
    let mut codec_epoch = 0u64;
    let mut last_presentation_timestamp_us = 0u64;
    while let Some((frame, mut force_keyframe)) = mailbox.take() {
        let dimensions = frame.dimensions();
        if native.as_ref().map(NativeEncoder::dimensions) != Some(dimensions) {
            native = None;
            codec_epoch = codec_epoch.saturating_add(1);
            match NativeEncoder::new(
                dimensions.0,
                dimensions.1,
                max_fps,
                target_bitrate_kbps,
                sink.clone(),
            ) {
                Ok(encoder) => native = Some(encoder),
                Err(error) => {
                    sink.on_event(CaptureEvent::Suspended(format!(
                        "VideoToolbox encoder creation failed: {error}"
                    )));
                    continue;
                }
            }
            force_keyframe = true;
        }
        let Some(native_encoder) = native.as_mut() else {
            continue;
        };
        let capture_timestamp_us = frame.capture_timestamp_us();
        let presentation_timestamp_us =
            capture_timestamp_us.max(last_presentation_timestamp_us.saturating_add(1));
        last_presentation_timestamp_us = presentation_timestamp_us;
        let metadata = FrameMetadata {
            codec_epoch,
            width_px: dimensions.0,
            height_px: dimensions.1,
            capture_timestamp_us,
            presentation_timestamp_us,
            encode_started_at: Instant::now(),
        };
        let result = match frame {
            EncoderInput::Owned(frame) => native_encoder.encode_bgra(
                &frame.bytes,
                frame.bytes_per_row.unwrap_or_default(),
                metadata,
                force_keyframe,
                max_fps,
            ),
            EncoderInput::PixelBuffer { buffer, .. } => {
                native_encoder.encode_pixel_buffer(&buffer, metadata, force_keyframe, max_fps)
            }
        };
        if let Err(error) = result {
            sink.on_event(CaptureEvent::Suspended(format!(
                "VideoToolbox frame submission failed: {error}"
            )));
            native = None;
        }
    }
}

struct CallbackContext {
    sink: Arc<dyn CaptureSink>,
    failed: AtomicBool,
}

struct FrameMetadata {
    codec_epoch: u64,
    width_px: u32,
    height_px: u32,
    capture_timestamp_us: u64,
    presentation_timestamp_us: u64,
    encode_started_at: Instant,
}

struct NativeEncoder {
    session: NonNull<c_void>,
    context: Box<CallbackContext>,
    width: u32,
    height: u32,
}

impl NativeEncoder {
    fn new(
        width: u32,
        height: u32,
        max_fps: u16,
        target_bitrate_kbps: Option<u32>,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Self, String> {
        let mut context = Box::new(CallbackContext {
            sink,
            failed: AtomicBool::new(false),
        });
        let mut session = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSessionCreate(
                std::ptr::null(),
                width as i32,
                height as i32,
                CODEC_H264,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                Some(compression_output_callback),
                (&mut *context as *mut CallbackContext).cast(),
                &mut session,
            )
        };
        let session = NonNull::new(session)
            .filter(|_| status == 0)
            .ok_or_else(|| format!("VTCompressionSessionCreate returned {status}"))?;
        let mut encoder = Self {
            session,
            context,
            width,
            height,
        };
        encoder.configure(max_fps, target_bitrate_kbps)?;
        Ok(encoder)
    }

    const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn configure(&mut self, fps: u16, target_bitrate_kbps: Option<u32>) -> Result<(), String> {
        self.set_property_bool(unsafe { kVTCompressionPropertyKey_RealTime }, true)?;
        self.set_property_bool(
            unsafe { kVTCompressionPropertyKey_AllowFrameReordering },
            false,
        )?;
        self.set_property_i32(
            unsafe { kVTCompressionPropertyKey_ExpectedFrameRate },
            i32::from(fps),
        )?;
        let bitrate = target_bitrate_kbps.map_or_else(
            || {
                ((u64::from(self.width) * u64::from(self.height) * u64::from(fps) * 12) / 100)
                    .clamp(500_000, 8_000_000)
            },
            |kilobits| u64::from(kilobits).saturating_mul(1_000),
        );
        let bitrate = i32::try_from(bitrate).unwrap_or(i32::MAX);
        self.set_property_i32(unsafe { kVTCompressionPropertyKey_AverageBitRate }, bitrate)?;
        self.set_property_i32(
            unsafe { kVTCompressionPropertyKey_MaxKeyFrameInterval },
            i32::from(fps).saturating_mul(2),
        )?;
        self.set_property_raw(unsafe { kVTCompressionPropertyKey_ProfileLevel }, unsafe {
            kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel
        })?;
        let status = unsafe { VTCompressionSessionPrepareToEncodeFrames(self.session.as_ptr()) };
        (status == 0)
            .then_some(())
            .ok_or_else(|| format!("VTCompressionSessionPrepareToEncodeFrames returned {status}"))
    }

    fn set_property_bool(&self, key: *const c_void, value: bool) -> Result<(), String> {
        self.set_property_raw(key, unsafe {
            if value {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            }
        })
    }

    fn set_property_i32(&self, key: *const c_void, value: i32) -> Result<(), String> {
        let number = unsafe {
            CFNumberCreate(
                std::ptr::null(),
                CF_NUMBER_SINT32,
                (&value as *const i32).cast(),
            )
        };
        let Some(number) = NonNull::new(number.cast_mut()) else {
            return Err("CFNumberCreate returned null".into());
        };
        let result = self.set_property_raw(key, number.as_ptr());
        unsafe { CFRelease(number.as_ptr()) };
        result
    }

    fn set_property_raw(&self, key: *const c_void, value: *const c_void) -> Result<(), String> {
        let status = unsafe { VTSessionSetProperty(self.session.as_ptr(), key, value) };
        (status == 0)
            .then_some(())
            .ok_or_else(|| format!("VTSessionSetProperty returned {status}"))
    }

    fn encode_bgra(
        &mut self,
        bytes: &[u8],
        source_stride: u32,
        metadata: FrameMetadata,
        force_keyframe: bool,
        fps: u16,
    ) -> Result<(), String> {
        let packed_stride = self.width as usize * 4;
        let source_stride = source_stride as usize;
        let needed = source_stride.saturating_mul(self.height as usize);
        if source_stride < packed_stride || bytes.len() < needed {
            return Err("BGRA payload is smaller than its declared geometry".into());
        }
        let pixel_buffer =
            CVPixelBuffer::create(self.width as usize, self.height as usize, PIXEL_FORMAT_BGRA)
                .map_err(|status| format!("CVPixelBuffer::create returned {status}"))?;
        {
            let mut guard = pixel_buffer
                .lock_read_write()
                .map_err(|status| format!("CVPixelBuffer lock returned {status}"))?;
            let destination_stride = guard.bytes_per_row();
            let destination = guard
                .as_slice_mut()
                .ok_or_else(|| "CVPixelBuffer had no writable base address".to_owned())?;
            for row in 0..self.height as usize {
                let source_start = row * source_stride;
                let destination_start = row * destination_stride;
                destination[destination_start..destination_start + packed_stride]
                    .copy_from_slice(&bytes[source_start..source_start + packed_stride]);
            }
        }

        self.encode_pixel_buffer(&pixel_buffer, metadata, force_keyframe, fps)
    }

    fn encode_pixel_buffer(
        &mut self,
        pixel_buffer: &CVPixelBuffer,
        metadata: FrameMetadata,
        force_keyframe: bool,
        fps: u16,
    ) -> Result<(), String> {
        if self.context.failed.swap(false, Ordering::AcqRel) {
            return Err("VideoToolbox asynchronous output failed".into());
        }
        let dimensions = (
            u32::try_from(pixel_buffer.width()).map_err(|_| "pixel buffer width exceeds u32")?,
            u32::try_from(pixel_buffer.height()).map_err(|_| "pixel buffer height exceeds u32")?,
        );
        if dimensions != self.dimensions() || pixel_buffer.pixel_format() != PIXEL_FORMAT_BGRA {
            return Err("pixel buffer format or dimensions differ from encoder session".into());
        }
        let frame_properties = force_keyframe_dictionary(force_keyframe);
        let metadata = Box::into_raw(Box::new(metadata));
        let timestamp = CMTimeRaw::new(
            i64::try_from(unsafe { (*metadata).presentation_timestamp_us }).unwrap_or(i64::MAX),
            1_000_000,
        );
        let duration = CMTimeRaw::new(1, i32::from(fps.max(1)));
        let status = unsafe {
            VTCompressionSessionEncodeFrame(
                self.session.as_ptr(),
                pixel_buffer.as_ptr(),
                timestamp,
                duration,
                frame_properties.unwrap_or(std::ptr::null()),
                metadata.cast(),
                std::ptr::null_mut(),
            )
        };
        if let Some(dictionary) = frame_properties {
            unsafe { CFRelease(dictionary) };
        }
        if status == 0 {
            Ok(())
        } else {
            unsafe { drop(Box::from_raw(metadata)) };
            Err(format!("VTCompressionSessionEncodeFrame returned {status}"))
        }
    }
}

impl Drop for NativeEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = VTCompressionSessionCompleteFrames(self.session.as_ptr(), CMTimeRaw::INVALID);
            VTCompressionSessionInvalidate(self.session.as_ptr());
            CFRelease(self.session.as_ptr());
        }
        let _ = &self.context;
    }
}

fn force_keyframe_dictionary(force: bool) -> Option<*const c_void> {
    if !force {
        return None;
    }
    let keys = [unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }];
    let values = [unsafe { kCFBooleanTrue }];
    NonNull::new(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            std::ptr::null(),
            std::ptr::null(),
        )
        .cast_mut()
    })
    .map(|dictionary| dictionary.as_ptr().cast_const())
}

unsafe extern "C" fn compression_output_callback(
    output_refcon: *mut c_void,
    source_refcon: *mut c_void,
    status: i32,
    _info_flags: u32,
    sample_buffer: *mut c_void,
) {
    let metadata = (!source_refcon.is_null())
        .then(|| unsafe { Box::from_raw(source_refcon.cast::<FrameMetadata>()) });
    let Some(metadata) = metadata else {
        return;
    };
    if output_refcon.is_null() {
        return;
    }
    let context = unsafe { &*output_refcon.cast::<CallbackContext>() };
    if status != 0 || sample_buffer.is_null() {
        context.failed.store(true, Ordering::Release);
        context.sink.on_event(CaptureEvent::Suspended(format!(
            "VideoToolbox output callback failed with status {status}"
        )));
        return;
    }
    let Some(sample) = (unsafe { CMSampleBuffer::from_raw_retained(sample_buffer) }) else {
        context.failed.store(true, Ordering::Release);
        return;
    };
    let Some((bytes, keyframe)) = encoded_annex_b(&sample) else {
        context.failed.store(true, Ordering::Release);
        context.sink.on_event(CaptureEvent::Suspended(
            "VideoToolbox produced an invalid H.264 access unit".into(),
        ));
        return;
    };
    context.sink.on_event(CaptureEvent::Frame(OwnedFrame {
        bytes: bytes.into(),
        format: PixelFormat::H264AnnexB,
        width_px: metadata.width_px,
        height_px: metadata.height_px,
        bytes_per_row: None,
        capture_timestamp_us: metadata.capture_timestamp_us,
        encode_duration_us: Some(
            u32::try_from(metadata.encode_started_at.elapsed().as_micros()).unwrap_or(u32::MAX),
        ),
        codec_epoch: metadata.codec_epoch,
        keyframe,
    }));
}

fn encoded_annex_b(sample: &CMSampleBuffer) -> Option<(Vec<u8>, bool)> {
    let format = sample.format_description()?;
    let mut first_parameter_set = std::ptr::null();
    let mut first_parameter_set_size = 0usize;
    let mut parameter_set_count = 0usize;
    let mut nal_header_length = 0i32;
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format.as_ptr(),
            0,
            &mut first_parameter_set,
            &mut first_parameter_set_size,
            &mut parameter_set_count,
            &mut nal_header_length,
        )
    };
    if status != 0 || !(1..=4).contains(&nal_header_length) {
        return None;
    }
    let block = sample.data_buffer()?;
    let avcc = block.copy_data_bytes(0, block.data_length())?;
    let (mut annex_b, keyframe) = avcc_to_annex_b(&avcc, nal_header_length as usize)?;
    if keyframe {
        let mut config = Vec::new();
        for index in 0..parameter_set_count {
            let mut pointer = std::ptr::null();
            let mut size = 0usize;
            let parameter_status = unsafe {
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format.as_ptr(),
                    index,
                    &mut pointer,
                    &mut size,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if parameter_status != 0 || pointer.is_null() || size == 0 {
                return None;
            }
            config.extend_from_slice(&[0, 0, 0, 1]);
            config.extend_from_slice(unsafe { std::slice::from_raw_parts(pointer, size) });
        }
        config.append(&mut annex_b);
        annex_b = config;
    }
    Some((annex_b, keyframe))
}

fn avcc_to_annex_b(avcc: &[u8], nal_header_length: usize) -> Option<(Vec<u8>, bool)> {
    if !(1..=4).contains(&nal_header_length) {
        return None;
    }
    let mut offset = 0usize;
    let mut output = Vec::with_capacity(avcc.len().saturating_add(16));
    let mut keyframe = false;
    while offset < avcc.len() {
        let header_end = offset.checked_add(nal_header_length)?;
        let header = avcc.get(offset..header_end)?;
        let length = header
            .iter()
            .fold(0usize, |value, byte| (value << 8) | usize::from(*byte));
        offset = header_end;
        let end = offset.checked_add(length)?;
        let nal = avcc.get(offset..end)?;
        if nal.is_empty() {
            return None;
        }
        keyframe |= nal[0] & 0x1f == 5;
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
        offset = end;
    }
    (!output.is_empty()).then_some((output, keyframe))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn avcc_conversion_marks_idr_and_rejects_truncation() {
        let input = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 2, 0x41, 0xbb];
        let (output, keyframe) = avcc_to_annex_b(&input, 4).unwrap();
        assert!(keyframe);
        assert_eq!(output, [0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41, 0xbb]);
        assert!(avcc_to_annex_b(&input[..5], 4).is_none());
    }

    #[test]
    fn mailbox_replaces_obsolete_frames_and_forces_recovery() {
        let mailbox = EncoderMailbox::new();
        mailbox.submit(EncoderInput::Owned(frame(1, 64, 64)));
        mailbox.submit(EncoderInput::Owned(frame(2, 64, 64)));
        let (latest, force) = mailbox.take().unwrap();
        assert_eq!(latest.capture_timestamp_us(), 2);
        assert!(force);
        assert_eq!(
            mailbox
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replaced_frames,
            1
        );
    }

    #[test]
    fn mailbox_retains_the_native_pixel_buffer_without_copying() {
        let mailbox = EncoderMailbox::new();
        let buffer = CVPixelBuffer::create(64, 64, PIXEL_FORMAT_BGRA).unwrap();
        let expected = buffer.as_ptr();
        mailbox.submit(EncoderInput::PixelBuffer {
            buffer,
            width_px: 64,
            height_px: 64,
            capture_timestamp_us: 7,
        });

        let (latest, force) = mailbox.take().unwrap();

        let EncoderInput::PixelBuffer { buffer, .. } = latest else {
            panic!("expected a native pixel buffer");
        };
        assert_eq!(buffer.as_ptr(), expected);
        assert!(!force);
    }

    #[test]
    fn keyframe_request_replays_the_last_static_frame() {
        let mailbox = EncoderMailbox::new();
        mailbox.submit(EncoderInput::Owned(frame(7, 64, 64)));
        let (initial, force) = mailbox.take().unwrap();
        assert_eq!(initial.capture_timestamp_us(), 7);
        assert!(!force);

        mailbox.request_keyframe();
        let (replayed, force) = mailbox.take().unwrap();
        assert_eq!(replayed.capture_timestamp_us(), 7);
        assert!(force);
    }

    struct Sink(mpsc::Sender<OwnedFrame>);

    impl CaptureSink for Sink {
        fn on_event(&self, event: CaptureEvent) {
            if let CaptureEvent::Frame(frame) = event {
                let _ = self.0.send(frame);
            }
        }
    }

    #[test]
    fn videotoolbox_encodes_idr_recovery_and_new_codec_epochs() {
        let (tx, rx) = mpsc::channel();
        let encoder = MacosH264Encoder::start(15, None, Arc::new(Sink(tx))).unwrap();
        encoder.on_event(CaptureEvent::Frame(frame(1, 64, 64)));
        // A cold VideoToolbox session can take seconds to emit its first
        // frame while the rest of the suite loads the machine.
        let encoded = match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(encoded) => encoded,
            Err(mpsc::RecvTimeoutError::Timeout) if std::env::var_os("CI").is_some() => {
                eprintln!(
                    "VideoToolbox produced no output on the headless CI runner; \
                     deterministic Annex B and mailbox tests still ran"
                );
                return;
            }
            Err(error) => panic!("VideoToolbox produced no output: {error:?}"),
        };
        assert_eq!(encoded.format, PixelFormat::H264AnnexB);
        assert!(encoded.keyframe);
        assert_eq!(encoded.codec_epoch, 1);
        assert!(encoded.bytes.starts_with(&[0, 0, 0, 1]));
        assert!(encoded.bytes.len() < 64 * 64 * 4);
        let nal_types = annex_b_nal_types(&encoded.bytes);
        assert!(nal_types.contains(&7), "keyframe omitted SPS");
        assert!(nal_types.contains(&8), "keyframe omitted PPS");
        assert!(nal_types.contains(&5), "keyframe omitted IDR");

        encoder.request_keyframe();
        let recovered = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("VideoToolbox did not replay the static recovery frame");
        assert!(recovered.keyframe);
        assert_eq!(recovered.codec_epoch, 1);

        encoder.on_event(CaptureEvent::Frame(frame(3, 32, 32)));
        let resized = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("VideoToolbox produced no resized frame");
        assert!(resized.keyframe);
        assert_eq!(resized.codec_epoch, 2);
    }

    fn frame(timestamp: u64, width: u32, height: u32) -> OwnedFrame {
        OwnedFrame {
            bytes: vec![0x80; width as usize * height as usize * 4].into(),
            format: PixelFormat::Bgra8,
            width_px: width,
            height_px: height,
            bytes_per_row: Some(width * 4),
            capture_timestamp_us: timestamp,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        }
    }

    fn annex_b_nal_types(bytes: &[u8]) -> Vec<u8> {
        let mut types = Vec::new();
        let mut offset = 0usize;
        while offset + 3 < bytes.len() {
            let header = if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
                Some(offset + 4)
            } else if bytes[offset..].starts_with(&[0, 0, 1]) {
                Some(offset + 3)
            } else {
                None
            };
            if let Some(header) = header {
                if let Some(byte) = bytes.get(header) {
                    types.push(byte & 0x1f);
                }
            }
            offset += 1;
        }
        types
    }
}
