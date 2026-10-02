// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Codec-neutral value types: codecs, backends, frames, configuration and
//! encoded output.

use serde::{Deserialize, Serialize};

/// Compressed video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// H.264 / AVC, Annex B byte stream.
    H264,
    /// H.265 / HEVC, Annex B byte stream. Hardware/OS encoders only (patent
    /// pools make a software HEVC encoder a poor default).
    Hevc,
    /// AV1, low-overhead OBU stream (Section 5 of the AV1 spec).
    Av1,
}

impl VideoCodec {
    /// Every codec, in the default preference order (most compatible first).
    pub const ALL: [VideoCodec; 3] = [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1];

    /// WebCodecs-style short name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }
}

/// An encoder or decoder implementation.
///
/// The declaration order is the probing priority for encoders: fastest
/// first (plan §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// NVIDIA NVENC / NVDEC (Video Codec SDK, dynamically loaded).
    Nvenc,
    /// VA-API (Intel/AMD on Linux, libva dynamically loaded).
    Vaapi,
    /// Intel Quick Sync through the oneVPL dispatcher.
    Qsv,
    /// AMD Advanced Media Framework.
    Amf,
    /// Apple VideoToolbox.
    #[serde(rename = "videotoolbox")]
    VideoToolbox,
    /// Windows Media Foundation hardware MFTs.
    MediaFoundation,
    /// Cisco OpenH264 (software, BSD).
    #[serde(rename = "openh264")]
    OpenH264,
    /// rav1e AV1 software encoder (BSD).
    Rav1e,
    /// dav1d AV1 software decoder (BSD).
    Dav1d,
    /// Test/injected backend.
    Fake,
}

impl Backend {
    /// Encoder probe order, fastest first.
    pub const ENCODER_PRIORITY: [Backend; 8] = [
        Backend::Nvenc,
        Backend::Vaapi,
        Backend::Qsv,
        Backend::Amf,
        Backend::VideoToolbox,
        Backend::MediaFoundation,
        Backend::OpenH264,
        Backend::Rav1e,
    ];

    /// Position in [`Self::ENCODER_PRIORITY`] (lower is preferred).
    pub fn priority(self) -> usize {
        Self::ENCODER_PRIORITY
            .iter()
            .position(|b| *b == self)
            .unwrap_or(Self::ENCODER_PRIORITY.len())
    }

    /// True for GPU / fixed-function implementations.
    pub fn is_hardware(self) -> bool {
        matches!(
            self,
            Self::Nvenc
                | Self::Vaapi
                | Self::Qsv
                | Self::Amf
                | Self::VideoToolbox
                | Self::MediaFoundation
        )
    }

    /// Environment variable that enables real-hardware tests for this backend.
    pub fn hardware_test_env(self) -> Option<&'static str> {
        match self {
            Self::Nvenc => Some("CUA_CODEC_TEST_NVENC"),
            Self::Vaapi => Some("CUA_CODEC_TEST_VAAPI"),
            Self::Qsv => Some("CUA_CODEC_TEST_QSV"),
            Self::Amf => Some("CUA_CODEC_TEST_AMF"),
            Self::MediaFoundation => Some("CUA_CODEC_TEST_MF"),
            _ => None,
        }
    }
}

/// Coarse latency ranking used by [`crate::select::select`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    /// Fixed-function hardware, sub-frame encode at 1080p.
    Hardware,
    /// OS-mediated hardware with extra copies or no on-demand IDR.
    OsHardware,
    /// Real-time software (a few ms at 720p on one core).
    SoftwareRealtime,
    /// Software that cannot keep 1080p30 in real time on typical hosts.
    SoftwareSlow,
}

/// Uncompressed pixel layout accepted by [`VideoFrame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PixelFormat {
    /// 8-bit B, G, R, A (the native capture format on macOS and Windows).
    Bgra,
    /// 8-bit Y plane + interleaved UV plane, 4:2:0.
    Nv12,
    /// 8-bit planar Y, U, V, 4:2:0.
    I420,
}

/// YCbCr matrix and range the encoder signals and the converters use.
///
/// The whole stack uses BT.709 limited range; clients should configure it
/// explicitly (browsers ignore the bitstream range flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    /// ITU-R BT.709, limited (video) range.
    #[default]
    Bt709Limited,
}

/// A Linux DMA-BUF frame handle (single or multi-plane).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaBufPlane {
    /// File descriptor (not owned; the caller keeps it open during encode).
    pub fd: i32,
    /// Byte offset of the plane.
    pub offset: u32,
    /// Row pitch in bytes.
    pub pitch: u32,
}

/// A DMA-BUF surface description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmaBufFrame {
    /// DRM fourcc (for example `NV12` or `AR24`).
    pub fourcc: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Planes, 1 to 4.
    pub planes: Vec<DmaBufPlane>,
}

/// A retained `CVPixelBufferRef` (macOS). The wrapper owns one retain count.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct CvPixelBuffer(std::ptr::NonNull<std::ffi::c_void>);

#[cfg(target_os = "macos")]
// SAFETY: CVPixelBuffer is a thread-safe CoreFoundation type; we only hand the
// pointer to VideoToolbox and retain/release it.
unsafe impl Send for CvPixelBuffer {}
#[cfg(target_os = "macos")]
// SAFETY: see above.
unsafe impl Sync for CvPixelBuffer {}

#[cfg(target_os = "macos")]
impl CvPixelBuffer {
    /// Wraps a `CVPixelBufferRef`, taking an additional retain.
    ///
    /// # Safety
    /// `ptr` must be a valid `CVPixelBufferRef`.
    pub unsafe fn retain(ptr: *mut std::ffi::c_void) -> Option<Self> {
        let ptr = std::ptr::NonNull::new(ptr)?;
        unsafe { crate::backends::videotoolbox::ffi::CFRetain(ptr.as_ptr()) };
        Some(Self(ptr))
    }

    /// Wraps a `CVPixelBufferRef` whose retain count the caller transfers.
    ///
    /// # Safety
    /// `ptr` must be a valid, +1 retained `CVPixelBufferRef`.
    pub unsafe fn from_retained(ptr: *mut std::ffi::c_void) -> Option<Self> {
        std::ptr::NonNull::new(ptr).map(Self)
    }

    /// The raw `CVPixelBufferRef`.
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0.as_ptr()
    }
}

#[cfg(target_os = "macos")]
impl Clone for CvPixelBuffer {
    fn clone(&self) -> Self {
        unsafe { crate::backends::videotoolbox::ffi::CFRetain(self.0.as_ptr()) };
        Self(self.0)
    }
}

#[cfg(target_os = "macos")]
impl Drop for CvPixelBuffer {
    fn drop(&mut self) {
        unsafe { crate::backends::videotoolbox::ffi::CFRelease(self.0.as_ptr()) };
    }
}

/// Pixel storage for one input frame.
#[derive(Debug, Clone, Copy)]
pub enum FrameData<'a> {
    /// Packed BGRA with `stride` bytes per row.
    Bgra {
        /// Pixel bytes.
        data: &'a [u8],
        /// Bytes per row (at least `width * 4`).
        stride: usize,
    },
    /// NV12.
    Nv12 {
        /// Luma plane.
        y: &'a [u8],
        /// Luma stride.
        y_stride: usize,
        /// Interleaved chroma plane.
        uv: &'a [u8],
        /// Chroma stride.
        uv_stride: usize,
    },
    /// Planar I420.
    I420 {
        /// Luma plane.
        y: &'a [u8],
        /// Cb plane.
        u: &'a [u8],
        /// Cr plane.
        v: &'a [u8],
        /// Strides of Y, U and V.
        strides: [usize; 3],
    },
    /// A GPU/IOSurface-backed pixel buffer (zero copy into VideoToolbox).
    #[cfg(target_os = "macos")]
    CvPixelBuffer(&'a CvPixelBuffer),
    /// A DMA-BUF (zero copy into VA-API/NVENC where supported). Backends that
    /// cannot import it return [`crate::CodecError::Unsupported`]; map it to
    /// CPU memory and retry.
    DmaBuf(&'a DmaBufFrame),
}

/// One uncompressed input frame.
#[derive(Debug, Clone, Copy)]
pub struct VideoFrame<'a> {
    /// Width in pixels (even for 4:2:0 formats).
    pub width: u32,
    /// Height in pixels (even for 4:2:0 formats).
    pub height: u32,
    /// Capture time on the shared media clock, microseconds (MEDIA.md §12.1).
    pub pts_us: u64,
    /// Pixel storage.
    pub data: FrameData<'a>,
}

impl<'a> VideoFrame<'a> {
    /// A packed BGRA frame with tight rows.
    pub fn bgra(width: u32, height: u32, pts_us: u64, data: &'a [u8]) -> Self {
        Self {
            width,
            height,
            pts_us,
            data: FrameData::Bgra {
                data,
                stride: width as usize * 4,
            },
        }
    }

    /// Pixel format of CPU-memory frames (`None` for native handles).
    pub fn pixel_format(&self) -> Option<PixelFormat> {
        match self.data {
            FrameData::Bgra { .. } => Some(PixelFormat::Bgra),
            FrameData::Nv12 { .. } => Some(PixelFormat::Nv12),
            FrameData::I420 { .. } => Some(PixelFormat::I420),
            _ => None,
        }
    }
}

/// How the encoder recovers from loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum KeyframePolicy {
    /// Infinite GOP; IDR only on [`crate::VideoEncoder::force_keyframe`]
    /// (and at start and after restarts). The default.
    IdrOnDemand,
    /// Gradual decoder refresh: a column of intra blocks sweeps the picture
    /// every `period_frames`. Backends without intra refresh fall back to
    /// `IdrOnDemand` and report a limitation.
    IntraRefresh {
        /// Frames per full refresh cycle.
        period_frames: u32,
    },
    /// Periodic IDR every `interval_frames` (for recording, or backends that
    /// cannot force an IDR, such as some Media Foundation MFTs).
    Periodic {
        /// Frames between IDRs.
        interval_frames: u32,
    },
}

/// Rate control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateControl {
    /// Constant bitrate with a VBV of about one frame. The default for
    /// streaming.
    Cbr,
    /// Capped variable bitrate (peak = 1.5x target).
    Vbr,
    /// Constant quality; `bitrate_kbps` is a ceiling hint only. Useful for the
    /// "lossless-ish text" mode.
    ConstantQuality {
        /// 0 (worst) to 100 (best).
        quality: u8,
    },
}

/// Latency/quality trade-off preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Fastest encode, lowest latency (the default).
    #[default]
    UltraLowLatency,
    /// Low latency with some quality tools enabled.
    LowLatency,
    /// Screen content with high quality (text), still no B-frames.
    Quality,
}

/// Encoder configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncoderConfig {
    /// Output codec.
    pub codec: VideoCodec,
    /// Coded width (even).
    pub width: u32,
    /// Coded height (even).
    pub height: u32,
    /// Nominal frame rate.
    pub fps: u32,
    /// Target bitrate in kbit/s.
    pub bitrate_kbps: u32,
    /// Rate control.
    pub rate_control: RateControl,
    /// Latency preset. B-frames are always off.
    pub preset: Preset,
    /// Loss recovery.
    pub keyframe_policy: KeyframePolicy,
    /// Largest access unit in bytes, 0 for no cap (should match transport
    /// FEC limits).
    pub max_frame_bytes: u32,
    /// Content hint: screen content (text, UI) rather than camera video.
    pub screen_content: bool,
    /// Worker threads for software encoders (0 = backend default).
    pub threads: u16,
}

impl EncoderConfig {
    /// A low-latency H.264 configuration with a pixel-count bitrate default
    /// (about 2 Mbit/s at 720p30, 4 Mbit/s at 1080p30).
    pub fn new(codec: VideoCodec, width: u32, height: u32, fps: u32) -> Self {
        let pixels = u64::from(width) * u64::from(height) * u64::from(fps.max(1));
        let bitrate_kbps = (pixels / 15_000).clamp(300, 50_000) as u32;
        Self {
            codec,
            width,
            height,
            fps: fps.max(1),
            bitrate_kbps,
            rate_control: RateControl::Cbr,
            preset: Preset::UltraLowLatency,
            keyframe_policy: KeyframePolicy::IdrOnDemand,
            max_frame_bytes: 0,
            screen_content: true,
            threads: 0,
        }
    }

    /// Builder: target bitrate.
    pub fn with_bitrate_kbps(mut self, kbps: u32) -> Self {
        self.bitrate_kbps = kbps;
        self
    }

    /// Validates dimensions and rates.
    pub fn validate(&self) -> crate::Result<()> {
        if self.width == 0
            || self.height == 0
            || !self.width.is_multiple_of(2)
            || !self.height.is_multiple_of(2)
        {
            return Err(crate::CodecError::InvalidArgument(format!(
                "dimensions must be non-zero and even, got {}x{}",
                self.width, self.height
            )));
        }
        if self.fps == 0 || self.fps > 240 {
            return Err(crate::CodecError::InvalidArgument(format!(
                "fps must be 1..=240, got {}",
                self.fps
            )));
        }
        if self.bitrate_kbps == 0 {
            return Err(crate::CodecError::InvalidArgument(
                "bitrate must be > 0".into(),
            ));
        }
        Ok(())
    }

    /// Fields whose change requires a new encoder session (a new codec
    /// epoch). Bitrate, fps and rate control are expected to change in place.
    pub fn needs_restart(&self, other: &Self) -> bool {
        self.codec != other.codec
            || self.width != other.width
            || self.height != other.height
            || self.preset != other.preset
            || self.keyframe_policy != other.keyframe_policy
            || self.screen_content != other.screen_content
            || self.threads != other.threads
    }
}

/// Outcome of [`crate::VideoEncoder::reconfigure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconfigured {
    /// Applied to the running session; the codec epoch is unchanged.
    InPlace,
    /// The session was rebuilt; the next access unit is an IDR with this new
    /// codec epoch.
    Restarted {
        /// The new epoch.
        codec_epoch: u64,
    },
}

/// One encoded access unit (one picture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    /// Annex B (H.264/HEVC, keyframes carry parameter sets in-band) or AV1
    /// low-overhead OBUs (keyframes carry a sequence header).
    pub data: Vec<u8>,
    /// Codec of `data`.
    pub codec: VideoCodec,
    /// True for IDR / AV1 key frames (independently decodable).
    pub keyframe: bool,
    /// Starts at 1; increments whenever the decoder must be reset (new
    /// session, size/codec change, backend fallback).
    pub codec_epoch: u64,
    /// Capture time of the source frame, microseconds.
    pub pts_us: u64,
    /// Coded width.
    pub width: u32,
    /// Coded height.
    pub height: u32,
    /// Submit-to-output time in microseconds.
    pub encode_duration_us: u32,
    /// Backend that produced it.
    pub backend: Backend,
}

/// A decoded picture in I420 (owned, tightly packed planes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Y plane, `width * height` bytes.
    pub y: Vec<u8>,
    /// U plane, `ceil(w/2) * ceil(h/2)` bytes.
    pub u: Vec<u8>,
    /// V plane, `ceil(w/2) * ceil(h/2)` bytes.
    pub v: Vec<u8>,
    /// Presentation time echoed from the input, microseconds.
    pub pts_us: u64,
}

impl DecodedFrame {
    /// Converts to packed BGRA (BT.709 limited range).
    pub fn to_bgra(&self) -> Vec<u8> {
        crate::convert::i420_to_bgra(self.width, self.height, &self.y, &self.u, &self.v)
    }
}

/// Microseconds on this process's monotonic media clock (MEDIA.md §12.1).
/// The origin is the first call in the process; the driver should use the
/// same function for video `capture_timestamp_us` so A/V share one clock.
pub fn media_clock_us() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros() as u64
}
