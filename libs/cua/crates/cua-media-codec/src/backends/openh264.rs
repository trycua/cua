// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! OpenH264 software H.264 encoder and decoder (BSD-2-Clause).
//!
//! Built from source by default. With the `openh264-cisco-binary` feature and
//! `CUA_OPENH264_LIBRARY=/path/to/libopenh264-<ver>.so|dylib|dll`, Cisco's
//! prebuilt binary is loaded instead (its SHA-256 is checked against the
//! known release list); Cisco's binary is the one covered by their H.264
//! patent licence. Download it at install time from
//! <http://ciscobinary.openh264.org/> (Cisco's terms forbid bundling it).

use std::time::Instant;

use openh264::decoder::Decoder;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig as OhConfig, FrameRate, FrameType,
    IntraFramePeriod, Profile, QpRange, RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{YUVSlices, YUVSource};
use openh264::OpenH264API;

use crate::convert::{self, I420Buffer};
use crate::error::{CodecError, Result};
use crate::types::{
    AccessUnit, Backend, DecodedFrame, EncoderConfig, FrameData, KeyframePolicy, Preset,
    RateControl, Reconfigured, VideoCodec, VideoFrame,
};
use crate::video::{EpochState, VideoDecoder, VideoEncoder};

const BACKEND: Backend = Backend::OpenH264;

/// Largest picture OpenH264 accepts (level 5.2).
pub const MAX_WIDTH: u32 = 3840;
/// Largest picture OpenH264 accepts (level 5.2).
pub const MAX_HEIGHT: u32 = 2160;

fn api() -> Result<OpenH264API> {
    #[cfg(feature = "openh264-cisco-binary")]
    if let Some(path) = std::env::var_os("CUA_OPENH264_LIBRARY") {
        return OpenH264API::from_blob_path(&path).map_err(|e| {
            CodecError::unavailable(
                BACKEND,
                format!("loading Cisco OpenH264 binary {path:?}: {e}"),
            )
        });
    }
    Ok(OpenH264API::from_source())
}

/// Human-readable description of the linked library.
pub fn library_description() -> &'static str {
    if cfg!(feature = "openh264-cisco-binary") && std::env::var_os("CUA_OPENH264_LIBRARY").is_some()
    {
        "Cisco OpenH264 binary (patent-licensed)"
    } else {
        "OpenH264 built from source (BSD-2-Clause; not covered by Cisco's patent licence)"
    }
}

fn native_config(cfg: &EncoderConfig) -> OhConfig {
    let (rc, qp) = match cfg.rate_control {
        RateControl::Cbr | RateControl::Vbr => (RateControlMode::Bitrate, QpRange::new(10, 45)),
        RateControl::ConstantQuality { quality } => {
            // quality 100 -> QP 12, quality 0 -> QP 45.
            let qp = 45 - (u32::from(quality.min(100)) * 33 / 100) as u8;
            (
                RateControlMode::Quality,
                QpRange::new(qp.saturating_sub(2), qp + 2),
            )
        }
    };
    let intra = match cfg.keyframe_policy {
        KeyframePolicy::Periodic { interval_frames } => interval_frames,
        // OpenH264 has no gradual decoder refresh; IntraRefresh degrades to
        // on-demand IDR (reported as a probe limitation).
        KeyframePolicy::IdrOnDemand | KeyframePolicy::IntraRefresh { .. } => 0,
    };
    let complexity = match cfg.preset {
        Preset::UltraLowLatency => Complexity::Low,
        Preset::LowLatency => Complexity::Medium,
        Preset::Quality => Complexity::High,
    };
    OhConfig::new()
        .bitrate(BitRate::from_bps(cfg.bitrate_kbps.saturating_mul(1000)))
        .max_frame_rate(FrameRate::from_hz(cfg.fps as f32))
        .rate_control_mode(rc)
        .qp(qp)
        .skip_frames(false)
        .usage_type(if cfg.screen_content {
            UsageType::ScreenContentRealTime
        } else {
            UsageType::CameraVideoRealTime
        })
        .profile(Profile::Baseline)
        .complexity(complexity)
        // Screen-content mode requires scene-change detection and does not
        // support adaptive quantisation or background detection.
        .scene_change_detect(cfg.screen_content)
        .adaptive_quantization(!cfg.screen_content)
        .background_detection(!cfg.screen_content)
        .intra_frame_period(IntraFramePeriod::from_num_frames(intra))
        .num_threads(cfg.threads)
        .vui(VuiConfig::bt709())
}

/// OpenH264 encoder session.
pub struct OpenH264Encoder {
    config: EncoderConfig,
    inner: Encoder,
    epoch: EpochState,
    scratch: I420Buffer,
}

impl OpenH264Encoder {
    /// Opens a session.
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        config.validate()?;
        if config.codec != VideoCodec::H264 {
            return Err(CodecError::Unsupported(format!(
                "OpenH264 encodes H.264 only, not {:?}",
                config.codec
            )));
        }
        if config.width.max(config.height) > MAX_WIDTH
            || config.width.min(config.height) > MAX_HEIGHT
        {
            return Err(CodecError::Unsupported(format!(
                "OpenH264 is limited to 3840x2160, got {}x{}",
                config.width, config.height
            )));
        }
        let inner = Encoder::with_api_config(api()?, native_config(config))
            .map_err(|e| CodecError::backend(BACKEND, e.to_string()))?;
        Ok(Self {
            config: config.clone(),
            inner,
            epoch: EpochState::new(),
            scratch: I420Buffer::new(config.width, config.height),
        })
    }

    fn restart(&mut self, config: &EncoderConfig) -> Result<u64> {
        let fresh = Self::new(config)?;
        self.inner = fresh.inner;
        self.config = config.clone();
        self.scratch = fresh.scratch;
        Ok(self.epoch.restart())
    }

    fn set_bitrate_in_place(&mut self, kbps: u32) -> Result<()> {
        let mut info = openh264_sys2::SBitrateInfo {
            iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
            iBitrate: i32::try_from(kbps.saturating_mul(1000)).unwrap_or(i32::MAX),
        };
        for option in [
            openh264_sys2::ENCODER_OPTION_BITRATE,
            openh264_sys2::ENCODER_OPTION_MAX_BITRATE,
        ] {
            // SAFETY: the encoder is initialized (constructed) and the option
            // pointer is a live SBitrateInfo for the duration of the call.
            let rc = unsafe {
                self.inner.raw_api().set_option(
                    option,
                    (&mut info as *mut openh264_sys2::SBitrateInfo).cast(),
                )
            };
            if rc != 0 {
                return Err(CodecError::backend(
                    BACKEND,
                    format!("SetOption(bitrate) returned {rc}"),
                ));
            }
        }
        Ok(())
    }

    fn set_fps_in_place(&mut self, fps: u32) -> Result<()> {
        let mut value = fps as f32;
        // SAFETY: as above; ENCODER_OPTION_FRAME_RATE takes a float.
        let rc = unsafe {
            self.inner.raw_api().set_option(
                openh264_sys2::ENCODER_OPTION_FRAME_RATE,
                (&mut value as *mut f32).cast(),
            )
        };
        if rc != 0 {
            return Err(CodecError::backend(
                BACKEND,
                format!("SetOption(frame rate) returned {rc}"),
            ));
        }
        Ok(())
    }
}

impl VideoEncoder for OpenH264Encoder {
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
        if config.needs_restart(&self.config) || config.rate_control != self.config.rate_control {
            let epoch = self.restart(config)?;
            return Ok(Reconfigured::Restarted { codec_epoch: epoch });
        }
        if config.bitrate_kbps != self.config.bitrate_kbps {
            self.set_bitrate_in_place(config.bitrate_kbps)?;
        }
        if config.fps != self.config.fps {
            self.set_fps_in_place(config.fps)?;
        }
        self.config = config.clone();
        Ok(Reconfigured::InPlace)
    }

    fn force_keyframe(&mut self) {
        self.epoch.force_keyframe = true;
    }

    fn encode(&mut self, frame: &VideoFrame<'_>) -> Result<Vec<AccessUnit>> {
        let started = Instant::now();
        if frame.width != self.config.width || frame.height != self.config.height {
            let mut next = self.config.clone();
            next.width = frame.width;
            next.height = frame.height;
            self.restart(&next)?;
        }
        let (w, h) = (frame.width, frame.height);
        let owned;
        let slices = match frame.data {
            FrameData::Bgra { data, stride } => {
                convert::bgra_to_i420_into(w, h, data, stride, &mut self.scratch);
                let cw = self.scratch.chroma_width();
                YUVSlices::new(
                    (&self.scratch.y, &self.scratch.u, &self.scratch.v),
                    (w as usize, h as usize),
                    (w as usize, cw, cw),
                )
            }
            FrameData::I420 { y, u, v, strides } => YUVSlices::new(
                (y, u, v),
                (w as usize, h as usize),
                (strides[0], strides[1], strides[2]),
            ),
            FrameData::Nv12 {
                y,
                y_stride,
                uv,
                uv_stride,
            } => {
                owned = convert::nv12_to_i420(w, h, y, y_stride, uv, uv_stride);
                let cw = owned.chroma_width();
                YUVSlices::new(
                    (&owned.y, &owned.u, &owned.v),
                    (w as usize, h as usize),
                    (w as usize, cw, cw),
                )
            }
            #[allow(unreachable_patterns)]
            _ => {
                return Err(CodecError::Unsupported(
                    "OpenH264 needs CPU memory (BGRA, NV12 or I420)".into(),
                ))
            }
        };
        if std::mem::take(&mut self.epoch.force_keyframe) {
            self.inner.force_intra_frame();
        }
        let ts = openh264::Timestamp::from_millis(frame.pts_us / 1000);
        let stream = self
            .inner
            .encode_at(&slices, ts)
            .map_err(|e| CodecError::backend(BACKEND, e.to_string()))?;
        let frame_type = stream.frame_type();
        if matches!(frame_type, FrameType::Skip | FrameType::Invalid) {
            return Ok(Vec::new());
        }
        let data = stream.to_vec();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let keyframe = matches!(frame_type, FrameType::IDR);
        Ok(vec![AccessUnit {
            data,
            codec: VideoCodec::H264,
            keyframe,
            codec_epoch: self.epoch.epoch,
            pts_us: frame.pts_us,
            width: w,
            height: h,
            encode_duration_us: u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
            backend: BACKEND,
        }])
    }
}

/// OpenH264 decoder.
pub struct OpenH264Decoder {
    inner: Decoder,
}

impl OpenH264Decoder {
    /// Creates a decoder.
    pub fn new() -> Result<Self> {
        let inner = Decoder::with_api_config(api()?, openh264::decoder::DecoderConfig::new())
            .map_err(|e| CodecError::backend(BACKEND, e.to_string()))?;
        Ok(Self { inner })
    }
}

impl VideoDecoder for OpenH264Decoder {
    fn is_hardware(&self) -> Option<bool> {
        Some(false)
    }

    fn backend(&self) -> Backend {
        BACKEND
    }

    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }

    fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Option<DecodedFrame>> {
        let Some(yuv) = self
            .inner
            .decode(data)
            .map_err(|e| CodecError::backend(BACKEND, e.to_string()))?
        else {
            return Ok(None);
        };
        let (w, h) = yuv.dimensions();
        let strides = yuv.strides();
        let buf = convert::i420_compact(
            w as u32,
            h as u32,
            yuv.y(),
            yuv.u(),
            yuv.v(),
            [strides.0, strides.1, strides.2],
        );
        Ok(Some(DecodedFrame {
            width: buf.width,
            height: buf.height,
            y: buf.y,
            u: buf.u,
            v: buf.v,
            pts_us,
        }))
    }

    fn reset(&mut self) -> Result<()> {
        *self = Self::new()?;
        Ok(())
    }
}

/// Real test encode used by the probe: 320x240 frame with a forced IDR must
/// produce an IDR access unit with SPS/PPS.
pub fn probe_encode() -> Result<()> {
    let cfg = EncoderConfig::new(VideoCodec::H264, 320, 240, 30).with_bitrate_kbps(500);
    let mut enc = OpenH264Encoder::new(&cfg)?;
    let pixels = vec![0x80u8; 320 * 240 * 4];
    let aus = enc.encode(&VideoFrame::bgra(320, 240, 0, &pixels))?;
    let au = aus
        .first()
        .ok_or_else(|| CodecError::backend(BACKEND, "probe encode produced no output"))?;
    if !au.keyframe || !crate::bitstream::has_parameter_sets(VideoCodec::H264, &au.data) {
        return Err(CodecError::backend(
            BACKEND,
            "probe encode did not start with IDR + SPS/PPS",
        ));
    }
    Ok(())
}
