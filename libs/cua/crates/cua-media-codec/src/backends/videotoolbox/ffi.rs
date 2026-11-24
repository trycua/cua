// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Minimal CoreFoundation / CoreVideo / CoreMedia / VideoToolbox bindings,
//! written from Apple's public SDK documentation. Only what the VideoToolbox
//! backend needs.
#![allow(non_upper_case_globals, non_snake_case, missing_docs, dead_code)]

use std::ffi::c_void;

pub type CFTypeRef = *const c_void;
pub type OSStatus = i32;

pub const kCFNumberSInt32Type: isize = 3;
pub const kCFNumberFloat32Type: isize = 5;
pub const kCVPixelFormatType_32BGRA: u32 = u32::from_be_bytes(*b"BGRA");
pub const kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: u32 = u32::from_be_bytes(*b"420v");
pub const kCMVideoCodecType_H264: u32 = u32::from_be_bytes(*b"avc1");
pub const kCMVideoCodecType_HEVC: u32 = u32::from_be_bytes(*b"hvc1");
pub const kCMTimeFlags_Valid: u32 = 1;
pub const kCVPixelBufferLock_ReadOnly: u64 = 1;
pub const kVTEncodeInfo_FrameDropped: u32 = 1 << 1;
pub const kCMBlockBufferAssureMemoryNowFlag: u32 = 1;

/// `CMTime` (CoreMedia packs it to 4-byte alignment).
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

impl CMTime {
    pub const INVALID: Self = Self {
        value: 0,
        timescale: 0,
        flags: 0,
        epoch: 0,
    };

    pub const fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: kCMTimeFlags_Valid,
            epoch: 0,
        }
    }
}

pub type VTCompressionOutputCallback = unsafe extern "C" fn(
    output_refcon: *mut c_void,
    source_frame_refcon: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    sample_buffer: *mut c_void,
);

pub type VTDecompressionOutputCallback = unsafe extern "C" fn(
    output_refcon: *mut c_void,
    source_frame_refcon: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    image_buffer: *mut c_void,
    presentation_timestamp: CMTime,
    presentation_duration: CMTime,
);

#[repr(C)]
pub struct VTDecompressionOutputCallbackRecord {
    pub callback: Option<VTDecompressionOutputCallback>,
    pub refcon: *mut c_void,
}

#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _private: [u8; 0],
}
#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _private: [u8; 0],
}
#[repr(C)]
pub struct CFArrayCallBacks {
    _private: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFNumberCreate(
        allocator: CFTypeRef,
        the_type: isize,
        value_ptr: *const c_void,
    ) -> CFTypeRef;
    pub fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFTypeRef;
    pub fn CFArrayCreate(
        allocator: CFTypeRef,
        values: *const CFTypeRef,
        num_values: isize,
        callbacks: *const CFArrayCallBacks,
    ) -> CFTypeRef;
    pub static kCFBooleanTrue: CFTypeRef;
    pub static kCFBooleanFalse: CFTypeRef;
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    pub static kCFTypeArrayCallBacks: CFArrayCallBacks;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    pub fn CVPixelBufferCreate(
        allocator: CFTypeRef,
        width: usize,
        height: usize,
        pixel_format: u32,
        attributes: CFTypeRef,
        out: *mut *mut c_void,
    ) -> i32;
    pub fn CVPixelBufferPoolCreatePixelBuffer(
        allocator: CFTypeRef,
        pool: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    pub fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    pub fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    pub fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    pub fn CVPixelBufferGetBaseAddressOfPlane(buffer: *mut c_void, plane: usize) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRowOfPlane(buffer: *mut c_void, plane: usize) -> usize;
    pub fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    pub fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
    pub fn CVPixelBufferGetPixelFormatType(buffer: *mut c_void) -> u32;
    pub fn CVPixelBufferGetPlaneCount(buffer: *mut c_void) -> usize;
    pub static kCVPixelBufferPixelFormatTypeKey: CFTypeRef;
    pub static kCVPixelBufferWidthKey: CFTypeRef;
    pub static kCVPixelBufferHeightKey: CFTypeRef;
    pub static kCVPixelBufferIOSurfacePropertiesKey: CFTypeRef;
    pub static kCVImageBufferColorPrimaries_ITU_R_709_2: CFTypeRef;
    pub static kCVImageBufferTransferFunction_ITU_R_709_2: CFTypeRef;
    pub static kCVImageBufferYCbCrMatrix_ITU_R_709_2: CFTypeRef;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub fn CMSampleBufferGetFormatDescription(sample: *mut c_void) -> *mut c_void;
    pub fn CMSampleBufferGetDataBuffer(sample: *mut c_void) -> *mut c_void;
    pub fn CMBlockBufferGetDataLength(block: *mut c_void) -> usize;
    pub fn CMBlockBufferCopyDataBytes(
        block: *mut c_void,
        offset: usize,
        length: usize,
        destination: *mut c_void,
    ) -> OSStatus;
    pub fn CMBlockBufferCreateWithMemoryBlock(
        allocator: CFTypeRef,
        memory_block: *mut c_void,
        block_length: usize,
        block_allocator: CFTypeRef,
        custom_block_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn CMBlockBufferReplaceDataBytes(
        source: *const c_void,
        destination: *mut c_void,
        offset_into_destination: usize,
        data_length: usize,
    ) -> OSStatus;
    pub fn CMSampleBufferCreateReady(
        allocator: CFTypeRef,
        data_buffer: *mut c_void,
        format_description: *mut c_void,
        num_samples: isize,
        num_sample_timing_entries: isize,
        sample_timing_array: *const c_void,
        num_sample_size_entries: isize,
        sample_size_array: *const usize,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: *mut c_void,
        index: usize,
        ptr_out: *mut *const u8,
        size_out: *mut usize,
        count_out: *mut usize,
        nal_header_length_out: *mut i32,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        desc: *mut c_void,
        index: usize,
        ptr_out: *mut *const u8,
        size_out: *mut usize,
        count_out: *mut usize,
        nal_header_length_out: *mut i32,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_unit_header_length: i32,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_unit_header_length: i32,
        extensions: CFTypeRef,
        out: *mut *mut c_void,
    ) -> OSStatus;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    pub fn VTCompressionSessionCreate(
        allocator: CFTypeRef,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFTypeRef,
        source_image_buffer_attributes: CFTypeRef,
        compressed_data_allocator: CFTypeRef,
        output_callback: Option<VTCompressionOutputCallback>,
        output_callback_refcon: *mut c_void,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn VTCompressionSessionPrepareToEncodeFrames(session: *mut c_void) -> OSStatus;
    pub fn VTCompressionSessionGetPixelBufferPool(session: *mut c_void) -> *mut c_void;
    pub fn VTCompressionSessionEncodeFrame(
        session: *mut c_void,
        image_buffer: *mut c_void,
        pts: CMTime,
        duration: CMTime,
        frame_properties: CFTypeRef,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    pub fn VTCompressionSessionCompleteFrames(session: *mut c_void, until: CMTime) -> OSStatus;
    pub fn VTCompressionSessionInvalidate(session: *mut c_void);
    pub fn VTSessionSetProperty(session: *mut c_void, key: CFTypeRef, value: CFTypeRef)
        -> OSStatus;
    pub fn VTSessionCopyProperty(
        session: *mut c_void,
        key: CFTypeRef,
        allocator: CFTypeRef,
        value_out: *mut CFTypeRef,
    ) -> OSStatus;
    pub fn VTDecompressionSessionCreate(
        allocator: CFTypeRef,
        format_description: *mut c_void,
        decoder_specification: CFTypeRef,
        destination_image_buffer_attributes: CFTypeRef,
        output_callback: *const VTDecompressionOutputCallbackRecord,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample_buffer: *mut c_void,
        decode_flags: u32,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    pub fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> OSStatus;
    pub fn VTDecompressionSessionInvalidate(session: *mut c_void);
    pub fn VTIsHardwareDecodeSupported(codec_type: u32) -> u8;

    pub static kVTCompressionPropertyKey_RealTime: CFTypeRef;
    pub static kVTCompressionPropertyKey_AllowFrameReordering: CFTypeRef;
    pub static kVTCompressionPropertyKey_ExpectedFrameRate: CFTypeRef;
    pub static kVTCompressionPropertyKey_AverageBitRate: CFTypeRef;
    pub static kVTCompressionPropertyKey_DataRateLimits: CFTypeRef;
    pub static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFTypeRef;
    pub static kVTCompressionPropertyKey_ProfileLevel: CFTypeRef;
    pub static kVTCompressionPropertyKey_Quality: CFTypeRef;
    pub static kVTCompressionPropertyKey_MaxFrameDelayCount: CFTypeRef;
    pub static kVTCompressionPropertyKey_ColorPrimaries: CFTypeRef;
    pub static kVTCompressionPropertyKey_TransferFunction: CFTypeRef;
    pub static kVTCompressionPropertyKey_YCbCrMatrix: CFTypeRef;
    pub static kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality: CFTypeRef;
    pub static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: CFTypeRef;
    pub static kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder: CFTypeRef;
    pub static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: CFTypeRef;
    pub static kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel: CFTypeRef;
    pub static kVTProfileLevel_H264_ConstrainedHigh_AutoLevel: CFTypeRef;
    pub static kVTProfileLevel_HEVC_Main_AutoLevel: CFTypeRef;
    pub static kVTEncodeFrameOptionKey_ForceKeyFrame: CFTypeRef;
}

/// An owned CoreFoundation reference released on drop.
pub struct CfOwned(pub CFTypeRef);

impl CfOwned {
    pub fn new(ptr: CFTypeRef) -> Option<Self> {
        (!ptr.is_null()).then_some(Self(ptr))
    }
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

pub fn cf_i32(value: i32) -> CfOwned {
    CfOwned::new(unsafe {
        CFNumberCreate(
            std::ptr::null(),
            kCFNumberSInt32Type,
            (&value as *const i32).cast(),
        )
    })
    .expect("CFNumberCreate")
}

pub fn cf_f32(value: f32) -> CfOwned {
    CfOwned::new(unsafe {
        CFNumberCreate(
            std::ptr::null(),
            kCFNumberFloat32Type,
            (&value as *const f32).cast(),
        )
    })
    .expect("CFNumberCreate")
}

pub fn cf_bool(value: bool) -> CFTypeRef {
    unsafe {
        if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        }
    }
}

/// Builds a CFDictionary with CFType retain semantics.
pub fn cf_dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> CfOwned {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    CfOwned::new(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    })
    .expect("CFDictionaryCreate")
}

/// Builds a CFArray with CFType retain semantics.
pub fn cf_array(values: &[CFTypeRef]) -> CfOwned {
    CfOwned::new(unsafe {
        CFArrayCreate(
            std::ptr::null(),
            values.as_ptr(),
            values.len() as isize,
            &kCFTypeArrayCallBacks,
        )
    })
    .expect("CFArrayCreate")
}
