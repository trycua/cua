// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! VideoToolbox H.264 decoder for the native macOS proxy window.
//!
//! RCDP carries Annex-B access units. CoreMedia expects AVCC samples, so each
//! NAL unit is converted to a four-byte length prefix before synchronous,
//! real-time VideoToolbox decode. Codec epochs recreate decoder state and a
//! missing or failed keyframe is surfaced as one bounded recovery request.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;

use apple_cf::cm::{CMBlockBuffer, CMFormatDescription, CMSampleBuffer};
use apple_cf::raw;
use cua_media_protocol::{CodecEpoch, VideoFrameDescriptor};

use super::{NativeFrame, NativeFrameData};

const CF_NUMBER_SINT32: isize = 3;
const VT_DECODE_INFO_FRAME_DROPPED: u32 = 1 << 1;

type DecompressionOutputCallback = unsafe extern "C" fn(
    output_refcon: *mut c_void,
    source_refcon: *mut c_void,
    status: i32,
    info_flags: u32,
    image_buffer: *mut c_void,
    presentation_timestamp: raw::CMTime,
    presentation_duration: raw::CMTime,
);

#[repr(C)]
struct DecompressionOutputCallbackRecord {
    callback: Option<DecompressionOutputCallback>,
    output_refcon: *mut c_void,
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: *const c_void,
        video_format_description: *mut c_void,
        video_decoder_specification: *const c_void,
        destination_image_buffer_attributes: *const c_void,
        output_callback: *const DecompressionOutputCallbackRecord,
        decompression_session_out: *mut *mut c_void,
    ) -> i32;
    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample_buffer: *mut c_void,
        decode_flags: u32,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> i32;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
    fn VTSessionSetProperty(
        session: *mut c_void,
        property_key: *const c_void,
        property_value: *const c_void,
    ) -> i32;

    static kVTDecompressionPropertyKey_RealTime: *const c_void;
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
    fn CFRetain(value: *const c_void) -> *const c_void;
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: *const c_void;
    static kCVPixelBufferMetalCompatibilityKey: *const c_void;
    static kCVPixelBufferIOSurfacePropertiesKey: *const c_void;
}

pub(super) struct H264Decoder {
    native: Option<NativeDecoder>,
    codec_epoch: Option<CodecEpoch>,
    needs_keyframe: bool,
    keyframe_request_sent: bool,
}

impl H264Decoder {
    pub(super) const fn new() -> Self {
        Self {
            native: None,
            codec_epoch: None,
            needs_keyframe: false,
            keyframe_request_sent: false,
        }
    }

    pub(super) fn decode(
        &mut self,
        descriptor: VideoFrameDescriptor,
        payload: &[u8],
    ) -> Result<Option<NativeFrame>, String> {
        if self.codec_epoch != Some(descriptor.codec_epoch) {
            self.native = None;
            self.codec_epoch = Some(descriptor.codec_epoch);
            self.needs_keyframe = true;
            self.keyframe_request_sent = false;
        }

        let nals = annex_b_nals(payload);
        if nals.is_empty() {
            self.mark_recovery();
            return Err("access unit contains no Annex-B NAL units".into());
        }

        if self.native.is_none() {
            let sps = nals.iter().find(|nal| nal_type(nal) == Some(7));
            let pps = nals.iter().find(|nal| nal_type(nal) == Some(8));
            let (Some(sps), Some(pps)) = (sps, pps) else {
                self.needs_keyframe = true;
                return Ok(None);
            };
            self.native = Some(NativeDecoder::new(sps, pps)?);
            self.needs_keyframe = false;
            self.keyframe_request_sent = false;
        }

        let sample = avcc_sample(&nals)?;
        if sample.is_empty() {
            return Ok(None);
        }
        let metadata = FrameMetadata {
            width_px: descriptor.width_px,
            height_px: descriptor.height_px,
            geometry_epoch: descriptor.geometry_epoch,
            sequence: descriptor.sequence,
            capture_timestamp_us: descriptor.capture_timestamp_us,
        };
        match self
            .native
            .as_mut()
            .expect("decoder was created above")
            .decode(&sample, metadata)
        {
            Ok(frame) => Ok(Some(frame)),
            Err(error) => {
                self.mark_recovery();
                Err(error)
            }
        }
    }

    pub(super) fn take_keyframe_request(&mut self) -> bool {
        if self.needs_keyframe && !self.keyframe_request_sent {
            self.keyframe_request_sent = true;
            true
        } else {
            false
        }
    }

    fn mark_recovery(&mut self) {
        self.native = None;
        self.needs_keyframe = true;
        self.keyframe_request_sent = false;
    }
}

struct DecodeContext {
    output: Mutex<Option<Result<NativeFrame, String>>>,
}

struct FrameMetadata {
    width_px: u32,
    height_px: u32,
    geometry_epoch: cua_media_protocol::GeometryEpoch,
    sequence: cua_media_protocol::FrameSequence,
    capture_timestamp_us: u64,
}

pub(super) struct DecodedPixelBuffer {
    pixel_buffer: NonNull<c_void>,
}

impl DecodedPixelBuffer {
    pub(super) fn as_ptr(&self) -> *mut c_void {
        self.pixel_buffer.as_ptr()
    }
}

impl std::fmt::Debug for DecodedPixelBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("DecodedPixelBuffer")
            .field(&self.pixel_buffer)
            .finish()
    }
}

// CoreVideo pixel buffers are reference-counted, immutable while presented,
// and documented for use across capture/decode and rendering queues.
unsafe impl Send for DecodedPixelBuffer {}
unsafe impl Sync for DecodedPixelBuffer {}

impl Drop for DecodedPixelBuffer {
    fn drop(&mut self) {
        unsafe { CFRelease(self.pixel_buffer.as_ptr()) };
    }
}

struct NativeDecoder {
    session: NonNull<c_void>,
    _format: CMFormatDescription,
    context: Box<DecodeContext>,
}

impl NativeDecoder {
    fn new(sps: &[u8], pps: &[u8]) -> Result<Self, String> {
        let format = h264_format_description(sps, pps)?;
        let mut context = Box::new(DecodeContext {
            output: Mutex::new(None),
        });
        let callback = DecompressionOutputCallbackRecord {
            callback: Some(decompression_output_callback),
            output_refcon: (&mut *context as *mut DecodeContext).cast(),
        };
        let attributes = bgra_attributes()?;
        let mut session = std::ptr::null_mut();
        let status = unsafe {
            VTDecompressionSessionCreate(
                std::ptr::null(),
                format.as_ptr(),
                std::ptr::null(),
                attributes.dictionary.as_ptr(),
                &callback,
                &mut session,
            )
        };
        let session = NonNull::new(session);
        if status != 0 || session.is_none() {
            if let Some(session) = session {
                unsafe {
                    VTDecompressionSessionInvalidate(session.as_ptr());
                    CFRelease(session.as_ptr());
                }
            }
            return Err(format!("VTDecompressionSessionCreate returned {status}"));
        }
        let session = session.expect("session was checked above");
        let realtime_status = unsafe {
            VTSessionSetProperty(
                session.as_ptr(),
                kVTDecompressionPropertyKey_RealTime,
                kCFBooleanTrue,
            )
        };
        if realtime_status != 0 {
            unsafe {
                VTDecompressionSessionInvalidate(session.as_ptr());
                CFRelease(session.as_ptr());
            }
            return Err(format!(
                "setting VideoToolbox real-time decode returned {realtime_status}"
            ));
        }
        Ok(Self {
            session,
            _format: format,
            context,
        })
    }

    fn decode(&mut self, bytes: &[u8], metadata: FrameMetadata) -> Result<NativeFrame, String> {
        let block = CMBlockBuffer::create(bytes)
            .ok_or_else(|| "CMBlockBuffer allocation failed".to_owned())?;
        let sample = sample_buffer(
            &block,
            &self._format,
            bytes.len(),
            metadata.capture_timestamp_us,
        )?;
        *self
            .context
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let metadata = Box::into_raw(Box::new(metadata));
        let mut info_flags = 0;
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                self.session.as_ptr(),
                sample.as_ptr(),
                0,
                metadata.cast(),
                &mut info_flags,
            )
        };
        if status != 0 {
            unsafe { drop(Box::from_raw(metadata)) };
            return Err(format!(
                "VTDecompressionSessionDecodeFrame returned {status}"
            ));
        }
        self.context
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| "VideoToolbox returned without a synchronous output frame".to_owned())?
    }
}

impl Drop for NativeDecoder {
    fn drop(&mut self) {
        unsafe {
            VTDecompressionSessionInvalidate(self.session.as_ptr());
            CFRelease(self.session.as_ptr());
        }
    }
}

unsafe extern "C" fn decompression_output_callback(
    output_refcon: *mut c_void,
    source_refcon: *mut c_void,
    status: i32,
    info_flags: u32,
    image_buffer: *mut c_void,
    _presentation_timestamp: raw::CMTime,
    _presentation_duration: raw::CMTime,
) {
    if output_refcon.is_null() || source_refcon.is_null() {
        return;
    }
    let context = unsafe { &*(output_refcon.cast::<DecodeContext>()) };
    let metadata = unsafe { Box::from_raw(source_refcon.cast::<FrameMetadata>()) };
    let result = if status != 0 {
        Err(format!("VideoToolbox decode callback returned {status}"))
    } else if info_flags & VT_DECODE_INFO_FRAME_DROPPED != 0 || image_buffer.is_null() {
        Err("VideoToolbox dropped the frame".into())
    } else {
        retain_bgra(image_buffer, *metadata)
    };
    *context
        .output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
}

fn retain_bgra(image_buffer: *mut c_void, metadata: FrameMetadata) -> Result<NativeFrame, String> {
    let pixel_buffer = image_buffer.cast();
    let format = unsafe { raw::CVPixelBufferGetPixelFormatType(pixel_buffer) };
    if format != raw::kCVPixelFormatType_32BGRA {
        return Err(format!("VideoToolbox emitted pixel format {format:#x}"));
    }
    let width = unsafe { raw::CVPixelBufferGetWidth(pixel_buffer) };
    let height = unsafe { raw::CVPixelBufferGetHeight(pixel_buffer) };
    if width == 0 || height == 0 {
        return Err("VideoToolbox emitted an empty BGRA buffer".into());
    }
    if !decoded_dimension_matches(width, metadata.width_px)
        || !decoded_dimension_matches(height, metadata.height_px)
    {
        return Err(format!(
            "decoded dimensions {width}x{height} differ from descriptor {}x{}",
            metadata.width_px, metadata.height_px
        ));
    }
    let retained = unsafe { CFRetain(image_buffer) }.cast_mut();
    let pixel_buffer = NonNull::new(retained)
        .ok_or_else(|| "retaining the VideoToolbox pixel buffer failed".to_owned())?;
    Ok(NativeFrame {
        // H.264 4:2:0 requires even chroma dimensions. VideoToolbox may
        // therefore round an odd source edge down by one pixel while the
        // remote window geometry remains odd-sized. Render the actual
        // decoded surface and keep geometry/sequence for input correlation.
        width_px: u32::try_from(width).map_err(|_| "decoded width exceeds u32")?,
        height_px: u32::try_from(height).map_err(|_| "decoded height exceeds u32")?,
        data: NativeFrameData::PixelBuffer(DecodedPixelBuffer { pixel_buffer }),
        geometry_epoch: metadata.geometry_epoch,
        sequence: metadata.sequence,
        received_at: std::time::Instant::now(),
    })
}

fn decoded_dimension_matches(decoded: usize, declared: u32) -> bool {
    usize::try_from(declared).is_ok_and(|declared| decoded.abs_diff(declared) <= 1)
}

fn h264_format_description(sps: &[u8], pps: &[u8]) -> Result<CMFormatDescription, String> {
    let pointers = [sps.as_ptr(), pps.as_ptr()];
    let sizes = [sps.len(), pps.len()];
    let mut description: raw::CMFormatDescriptionRef = std::ptr::null();
    let status = unsafe {
        raw::CMVideoFormatDescriptionCreateFromH264ParameterSets(
            std::ptr::null(),
            pointers.len(),
            pointers.as_ptr(),
            sizes.as_ptr(),
            4,
            &mut description,
        )
    };
    CMFormatDescription::from_raw(description.cast_mut().cast())
        .filter(|_| status == 0)
        .ok_or_else(|| {
            format!("CMVideoFormatDescriptionCreateFromH264ParameterSets returned {status}")
        })
}

fn sample_buffer(
    block: &CMBlockBuffer,
    format: &CMFormatDescription,
    size: usize,
    capture_timestamp_us: u64,
) -> Result<CMSampleBuffer, String> {
    let timestamp = raw::CMTime {
        value: i64::try_from(capture_timestamp_us).unwrap_or(i64::MAX),
        timescale: 1_000_000,
        flags: raw::kCMTimeFlags_Valid,
        epoch: 0,
    };
    let timing = raw::CMSampleTimingInfo {
        duration: unsafe { raw::kCMTimeInvalid },
        presentationTimeStamp: timestamp,
        decodeTimeStamp: unsafe { raw::kCMTimeInvalid },
    };
    let mut sample = std::ptr::null_mut();
    let status = unsafe {
        raw::CMSampleBufferCreateReady(
            std::ptr::null(),
            block.as_ptr().cast(),
            format.as_ptr().cast(),
            1,
            1,
            &timing,
            1,
            &size,
            &mut sample,
        )
    };
    CMSampleBuffer::from_raw(sample.cast())
        .filter(|_| status == 0)
        .ok_or_else(|| format!("CMSampleBufferCreateReady returned {status}"))
}

struct BgraAttributes {
    dictionary: NonNull<c_void>,
    pixel_format_number: NonNull<c_void>,
    io_surface_properties: NonNull<c_void>,
}

impl Drop for BgraAttributes {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.dictionary.as_ptr());
            CFRelease(self.pixel_format_number.as_ptr());
            CFRelease(self.io_surface_properties.as_ptr());
        }
    }
}

fn bgra_attributes() -> Result<BgraAttributes, String> {
    let format = raw::kCVPixelFormatType_32BGRA as i32;
    let number = unsafe {
        CFNumberCreate(
            std::ptr::null(),
            CF_NUMBER_SINT32,
            (&format as *const i32).cast(),
        )
    };
    let Some(number) = NonNull::new(number.cast_mut()) else {
        return Err("CFNumberCreate returned null".into());
    };
    let io_surface_properties = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let Some(io_surface_properties) = NonNull::new(io_surface_properties.cast_mut()) else {
        unsafe { CFRelease(number.as_ptr()) };
        return Err("creating empty IOSurface properties failed".into());
    };
    let keys = [
        unsafe { kCVPixelBufferPixelFormatTypeKey },
        unsafe { kCVPixelBufferMetalCompatibilityKey },
        unsafe { kCVPixelBufferIOSurfacePropertiesKey },
    ];
    let values = [
        number.as_ptr().cast_const(),
        unsafe { kCFBooleanTrue },
        io_surface_properties.as_ptr().cast_const(),
    ];
    let dictionary = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let Some(dictionary) = NonNull::new(dictionary.cast_mut()) else {
        unsafe {
            CFRelease(io_surface_properties.as_ptr());
            CFRelease(number.as_ptr());
        }
        return Err("CFDictionaryCreate returned null".into());
    };
    Ok(BgraAttributes {
        dictionary,
        pixel_format_number: number,
        io_surface_properties,
    })
}

fn avcc_sample(nals: &[&[u8]]) -> Result<Vec<u8>, String> {
    let mut sample = Vec::new();
    for nal in nals {
        if matches!(nal_type(nal), Some(7..=9)) {
            continue;
        }
        let length = u32::try_from(nal.len()).map_err(|_| "H.264 NAL unit exceeds 4 GiB")?;
        sample.extend_from_slice(&length.to_be_bytes());
        sample.extend_from_slice(nal);
    }
    Ok(sample)
}

fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|byte| byte & 0x1f)
}

fn annex_b_nals(bytes: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let start_length = if index + 4 <= bytes.len() && bytes[index..index + 4] == [0, 0, 0, 1] {
            Some(4)
        } else if bytes[index..index + 3] == [0, 0, 1] {
            Some(3)
        } else {
            None
        };
        if let Some(length) = start_length {
            starts.push((index, index + length));
            index += length;
        } else {
            index += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .filter_map(|(position, &(_, payload_start))| {
            let end = starts
                .get(position + 1)
                .map_or(bytes.len(), |&(start, _)| start);
            (payload_start < end).then_some(&bytes[payload_start..end])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_parser_accepts_three_and_four_byte_start_codes() {
        let bytes = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5,
        ];
        let nals = annex_b_nals(&bytes);
        assert_eq!(nals, vec![&bytes[4..7], &bytes[10..12], &bytes[16..19]]);
        assert_eq!(nal_type(nals[0]), Some(7));
        assert_eq!(nal_type(nals[1]), Some(8));
        assert_eq!(nal_type(nals[2]), Some(5));
    }

    #[test]
    fn avcc_sample_uses_lengths_and_omits_parameter_sets() {
        let sps = [0x67, 1];
        let pps = [0x68, 2];
        let idr = [0x65, 3, 4];
        let sample = avcc_sample(&[&sps, &pps, &idr]).unwrap();
        assert_eq!(sample, [0, 0, 0, 3, 0x65, 3, 4]);
    }

    #[test]
    fn decoded_dimensions_allow_videotoolbox_chroma_alignment() {
        assert!(decoded_dimension_matches(750, 751));
        assert!(decoded_dimension_matches(751, 751));
        assert!(decoded_dimension_matches(752, 751));
        assert!(!decoded_dimension_matches(749, 751));
    }
}
