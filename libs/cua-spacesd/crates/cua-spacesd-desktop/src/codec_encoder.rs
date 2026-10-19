// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The session's [`FrameEncoder`] seam backed by `cua-media-codec`.
//!
//! `cua-media-codec` probes the encoders this machine can run (NVENC, VA-API,
//! QSV, AMF, VideoToolbox, Media Foundation, OpenH264), picks the fastest
//! one that produces H.264 for the viewer, and falls back at runtime when a
//! backend fails. This module only adapts types: CPU BGRA frames in, Annex B
//! access units out.

use std::sync::Arc;

use crate::codec_adapter::{CodecFrameEncoder, FrameEncoder as CodecEncoderTrait};
use cua_media_codec::{
    ClientDecoder, DecoderKind, EncoderConfig as CodecConfig, SelectionPrefs, VideoCodec,
};
use cua_spacesd_provider_api::{OwnedFrame, PixelFormat};
use cua_spacesd_session::media::encoder::{
    EncodedVideo, EncoderConfig, EncoderError, FrameEncoder, FrameEncoderFactory, RawVideoFrame,
};

/// Opens a `cua-media-codec` H.264 encoder per stream.
#[derive(Debug, Clone)]
pub struct CodecEncoderFactory {
    client: Vec<ClientDecoder>,
    prefs: SelectionPrefs,
}

impl Default for CodecEncoderFactory {
    fn default() -> Self {
        Self {
            // Every media-plane client decodes H.264 (WebCodecs, OpenH264,
            // VideoToolbox, Media Foundation); the wire carries H.264 only.
            client: vec![
                ClientDecoder::new(DecoderKind::WebCodecs, &[VideoCodec::H264], false),
                ClientDecoder::new(DecoderKind::OpenH264, &[VideoCodec::H264], false),
            ],
            prefs: SelectionPrefs {
                codec_order: vec![VideoCodec::H264],
                ..SelectionPrefs::from_env()
            },
        }
    }
}

impl FrameEncoderFactory for CodecEncoderFactory {
    fn name(&self) -> &'static str {
        "cua-media-codec"
    }

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn FrameEncoder>, EncoderError> {
        let encoders = cua_media_codec::probe();
        let mut codec_config = CodecConfig::new(
            VideoCodec::H264,
            config.width,
            config.height,
            u32::from(config.max_fps.max(1)),
        );
        codec_config.bitrate_kbps = config.bitrate_kbps;
        codec_config.screen_content = true;
        let encoder = CodecFrameEncoder::open(&encoders, &self.client, &self.prefs, &codec_config)
            .map_err(|error| EncoderError(error.to_string()))?;
        let backend = backend_name(&encoder);
        Ok(Box::new(CodecEncoder {
            inner: encoder,
            backend,
            fps: u32::from(config.max_fps.max(1)),
            bitrate_kbps: config.bitrate_kbps,
            last_epoch: None,
        }))
    }
}

fn backend_name(encoder: &CodecFrameEncoder) -> &'static str {
    use cua_media_codec::Backend;
    match encoder.backend() {
        Backend::Nvenc => "nvenc",
        Backend::Vaapi => "vaapi",
        Backend::Qsv => "qsv",
        Backend::Amf => "amf",
        Backend::VideoToolbox => "videotoolbox",
        Backend::MediaFoundation => "mediafoundation",
        Backend::OpenH264 => "openh264",
        Backend::Rav1e => "rav1e",
        _ => "codec",
    }
}

struct CodecEncoder {
    inner: CodecFrameEncoder,
    backend: &'static str,
    fps: u32,
    bitrate_kbps: u32,
    last_epoch: Option<u64>,
}

impl FrameEncoder for CodecEncoder {
    fn name(&self) -> &'static str {
        self.backend
    }

    fn encode(
        &mut self,
        frame: &RawVideoFrame<'_>,
        force_keyframe: bool,
    ) -> Result<Option<EncodedVideo>, EncoderError> {
        let rows = frame.height as usize;
        let stride = frame.stride as usize;
        let owned = OwnedFrame {
            bytes: Arc::from(&frame.bgra[..stride * rows]),
            format: PixelFormat::Bgra8,
            width_px: frame.width,
            height_px: frame.height,
            bytes_per_row: Some(frame.stride),
            capture_timestamp_us: frame.capture_us,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: false,
        };
        if force_keyframe {
            self.inner.request_keyframe();
        }
        let encoded = self
            .inner
            .encode_frame(&owned)
            .map_err(|error| EncoderError(error.to_string()))?;
        Ok(encoded.map(|encoded| {
            // A backend switch (runtime fallback) starts a new epoch; tell
            // the pipeline so decoders reset.
            let restart = self
                .last_epoch
                .is_some_and(|epoch| epoch != encoded.codec_epoch);
            self.last_epoch = Some(encoded.codec_epoch);
            self.backend = backend_name(&self.inner);
            EncodedVideo {
                data: encoded.bytes.to_vec(),
                keyframe: encoded.keyframe,
                restart,
            }
        }))
    }

    fn set_bitrate_kbps(&mut self, kbps: u32) {
        self.bitrate_kbps = kbps;
        let _ = self.inner.set_rate(self.bitrate_kbps, self.fps);
    }

    fn set_max_fps(&mut self, fps: u16) {
        self.fps = u32::from(fps.max(1));
        let _ = self.inner.set_rate(self.bitrate_kbps, self.fps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_an_h264_encoder_and_emits_an_idr_with_parameter_sets() {
        let factory = CodecEncoderFactory::default();
        let mut encoder = factory
            .create(&EncoderConfig {
                width: 64,
                height: 48,
                max_fps: 30,
                bitrate_kbps: 1_000,
                intra_refresh: false,
            })
            .expect("an H.264 encoder (OpenH264 at least) is available");
        let red = [0u8, 0, 255, 255].repeat(64 * 48);
        let frame = RawVideoFrame {
            bgra: &red,
            width: 64,
            height: 48,
            stride: 256,
            capture_us: 0,
        };
        let first = encoder.encode(&frame, true).unwrap().expect("first frame");
        assert!(first.keyframe);
        let nal_types: Vec<u8> = first
            .data
            .windows(4)
            .enumerate()
            .filter(|(_, w)| w[..3] == [0, 0, 1])
            .map(|(i, _)| first.data[i + 3] & 0x1f)
            .collect();
        assert!(
            nal_types.contains(&7) && nal_types.contains(&8) && nal_types.contains(&5),
            "{nal_types:?}"
        );
        let forced = loop {
            if let Some(out) = encoder.encode(&frame, true).unwrap() {
                break out;
            }
        };
        assert!(forced.keyframe);
    }
}
