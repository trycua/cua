// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Optional AV1 software paths: rav1e encoder (`av1-rav1e`) and dav1d
//! decoder (`av1-dav1d`). Both BSD-2-Clause.

#[cfg(feature = "av1-rav1e")]
pub use encoder::{probe_encode, Rav1eEncoder};

#[cfg(feature = "av1-dav1d")]
pub use decoder::Dav1dDecoder;

#[cfg(feature = "av1-rav1e")]
mod encoder {
    use std::time::Instant;

    use rav1e::prelude::*;

    use crate::convert::{self, I420Buffer};
    use crate::error::{CodecError, Result};
    use crate::types::{
        AccessUnit, Backend, EncoderConfig, FrameData, KeyframePolicy, Reconfigured, VideoCodec,
        VideoFrame,
    };
    use crate::video::{EpochState, VideoEncoder};

    const BACKEND: Backend = Backend::Rav1e;

    /// rav1e session (speed preset 10, low-latency, no lookahead).
    pub struct Rav1eEncoder {
        config: EncoderConfig,
        ctx: Context<u8>,
        epoch: EpochState,
        scratch: I420Buffer,
    }

    fn context(cfg: &EncoderConfig) -> Result<Context<u8>> {
        let mut speed = SpeedSettings::from_preset(10);
        speed.rdo_lookahead_frames = 1;
        let max_kf = match cfg.keyframe_policy {
            KeyframePolicy::Periodic { interval_frames } => u64::from(interval_frames.max(1)),
            _ => u64::from(u32::MAX),
        };
        let enc = rav1e::config::EncoderConfig {
            width: cfg.width as usize,
            height: cfg.height as usize,
            bit_depth: 8,
            chroma_sampling: ChromaSampling::Cs420,
            time_base: Rational::new(1, u64::from(cfg.fps)),
            low_latency: true,
            min_key_frame_interval: 0,
            max_key_frame_interval: max_kf,
            bitrate: i32::try_from(u64::from(cfg.bitrate_kbps) * 1000).unwrap_or(i32::MAX),
            speed_settings: speed,
            ..Default::default()
        };
        Config::new()
            .with_encoder_config(enc)
            .with_threads(cfg.threads as usize)
            .new_context()
            .map_err(|e| CodecError::backend(BACKEND, format!("{e:?}")))
    }

    impl Rav1eEncoder {
        /// Opens a session.
        pub fn new(config: &EncoderConfig) -> Result<Self> {
            config.validate()?;
            if config.codec != VideoCodec::Av1 {
                return Err(CodecError::Unsupported("rav1e encodes AV1 only".into()));
            }
            Ok(Self {
                config: config.clone(),
                ctx: context(config)?,
                epoch: EpochState::new(),
                scratch: I420Buffer::new(config.width, config.height),
            })
        }
    }

    impl VideoEncoder for Rav1eEncoder {
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
            // rav1e has no live reconfiguration: rebuild.
            self.ctx = context(config)?;
            self.config = config.clone();
            self.scratch = I420Buffer::new(config.width, config.height);
            Ok(Reconfigured::Restarted {
                codec_epoch: self.epoch.restart(),
            })
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
                self.reconfigure(&next)?;
            }
            let (w, h) = (frame.width, frame.height);
            match frame.data {
                FrameData::Bgra { data, stride } => {
                    convert::bgra_to_i420_into(w, h, data, stride, &mut self.scratch)
                }
                FrameData::I420 { y, u, v, strides } => {
                    self.scratch = convert::i420_compact(w, h, y, u, v, strides)
                }
                FrameData::Nv12 {
                    y,
                    y_stride,
                    uv,
                    uv_stride,
                } => self.scratch = convert::nv12_to_i420(w, h, y, y_stride, uv, uv_stride),
                #[allow(unreachable_patterns)]
                _ => return Err(CodecError::Unsupported("rav1e needs CPU memory".into())),
            }
            let mut f = self.ctx.new_frame();
            let cw = self.scratch.chroma_width();
            f.planes[0].copy_from_raw_u8(&self.scratch.y, w as usize, 1);
            f.planes[1].copy_from_raw_u8(&self.scratch.u, cw, 1);
            f.planes[2].copy_from_raw_u8(&self.scratch.v, cw, 1);
            let params = FrameParameters {
                frame_type_override: if std::mem::take(&mut self.epoch.force_keyframe) {
                    FrameTypeOverride::Key
                } else {
                    FrameTypeOverride::No
                },
                opaque: Some(Opaque::new(frame.pts_us)),
                t35_metadata: Box::new([]),
            };
            self.ctx
                .send_frame((f, params))
                .map_err(|e| CodecError::backend(BACKEND, format!("send_frame: {e:?}")))?;
            let mut out = Vec::new();
            loop {
                match self.ctx.receive_packet() {
                    Ok(packet) => {
                        let pts_us = packet
                            .opaque
                            .and_then(|o| o.downcast::<u64>().ok())
                            .map_or(frame.pts_us, |b| *b);
                        out.push(AccessUnit {
                            keyframe: packet.frame_type == FrameType::KEY,
                            data: packet.data,
                            codec: VideoCodec::Av1,
                            codec_epoch: self.epoch.epoch,
                            pts_us,
                            width: w,
                            height: h,
                            encode_duration_us: u32::try_from(started.elapsed().as_micros())
                                .unwrap_or(u32::MAX),
                            backend: BACKEND,
                        });
                    }
                    Err(EncoderStatus::Encoded) => continue,
                    Err(EncoderStatus::NeedMoreData) | Err(EncoderStatus::LimitReached) => break,
                    Err(e) => {
                        return Err(CodecError::backend(
                            BACKEND,
                            format!("receive_packet: {e:?}"),
                        ))
                    }
                }
            }
            Ok(out)
        }
    }

    /// Probe: a real 160x120 encode must produce a key frame with a sequence
    /// header within a few frames.
    pub fn probe_encode() -> Result<()> {
        let cfg = EncoderConfig::new(VideoCodec::Av1, 160, 120, 30).with_bitrate_kbps(300);
        let mut enc = Rav1eEncoder::new(&cfg)?;
        let px = vec![0x60u8; 160 * 120 * 4];
        for i in 0..8 {
            let aus = enc.encode(&VideoFrame::bgra(160, 120, i * 33_000, &px))?;
            if let Some(au) = aus.first() {
                if au.keyframe && crate::bitstream::av1_has_sequence_header(&au.data) {
                    return Ok(());
                }
                return Err(CodecError::backend(
                    BACKEND,
                    "first AV1 packet is not a key frame with a sequence header",
                ));
            }
        }
        Err(CodecError::backend(
            BACKEND,
            "rav1e produced no packet in 8 frames",
        ))
    }
}

#[cfg(feature = "av1-dav1d")]
mod decoder {
    use dav1d::PlanarImageComponent as C;

    use crate::convert;
    use crate::error::{CodecError, Result};
    use crate::types::{Backend, DecodedFrame, VideoCodec};
    use crate::video::VideoDecoder;

    /// dav1d decoder with frame delay 1 (low latency).
    pub struct Dav1dDecoder {
        inner: dav1d::Decoder,
    }

    impl Dav1dDecoder {
        /// Creates a decoder.
        pub fn new() -> Result<Self> {
            let mut settings = dav1d::Settings::new();
            settings.set_max_frame_delay(1);
            let inner = dav1d::Decoder::with_settings(&settings)
                .map_err(|e| CodecError::backend(Backend::Dav1d, format!("{e:?}")))?;
            Ok(Self { inner })
        }
    }

    impl VideoDecoder for Dav1dDecoder {
        fn is_hardware(&self) -> Option<bool> {
            Some(false)
        }

        fn backend(&self) -> Backend {
            Backend::Dav1d
        }
        fn codec(&self) -> VideoCodec {
            VideoCodec::Av1
        }
        fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Option<DecodedFrame>> {
            let err = |e: dav1d::Error| CodecError::backend(Backend::Dav1d, format!("{e:?}"));
            match self
                .inner
                .send_data(data.to_vec(), None, Some(pts_us as i64), None)
            {
                Ok(()) | Err(dav1d::Error::Again) => {}
                Err(e) => return Err(err(e)),
            }
            let pic = match self.inner.get_picture() {
                Ok(p) => p,
                Err(dav1d::Error::Again) => return Ok(None),
                Err(e) => return Err(err(e)),
            };
            if pic.bit_depth() != 8 {
                return Err(CodecError::Unsupported(
                    "only 8-bit AV1 is supported".into(),
                ));
            }
            let (w, h) = (pic.width(), pic.height());
            let buf = convert::i420_compact(
                w,
                h,
                &pic.plane(C::Y),
                &pic.plane(C::U),
                &pic.plane(C::V),
                [
                    pic.stride(C::Y) as usize,
                    pic.stride(C::U) as usize,
                    pic.stride(C::V) as usize,
                ],
            );
            Ok(Some(DecodedFrame {
                width: w,
                height: h,
                y: buf.y,
                u: buf.u,
                v: buf.v,
                pts_us,
            }))
        }
        fn reset(&mut self) -> Result<()> {
            self.inner.flush();
            Ok(())
        }
    }
}
