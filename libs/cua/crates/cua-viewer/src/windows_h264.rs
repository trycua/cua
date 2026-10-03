// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! OpenH264 decode path for the native Windows proxy window.

use cua_media_protocol::{CodecEpoch, VideoCodec, VideoFrameDescriptor};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource as _;

use super::{NativeFrame, NativeFrameData};

pub(super) struct H264Decoder {
    decoder: Option<Decoder>,
    codec_epoch: Option<CodecEpoch>,
    awaiting_keyframe: bool,
    request_keyframe: bool,
}

impl H264Decoder {
    pub(super) fn new() -> Self {
        Self {
            decoder: None,
            codec_epoch: None,
            awaiting_keyframe: true,
            request_keyframe: false,
        }
    }

    pub(super) fn decode(
        &mut self,
        descriptor: VideoFrameDescriptor,
        annex_b: &[u8],
    ) -> Result<Option<NativeFrame>, String> {
        if descriptor.codec != VideoCodec::H264 {
            return Err(format!("OpenH264 decoder received {:?}", descriptor.codec));
        }
        if self.codec_epoch != Some(descriptor.codec_epoch) {
            self.decoder = Some(Decoder::new().map_err(|error| error.to_string())?);
            self.codec_epoch = Some(descriptor.codec_epoch);
            self.awaiting_keyframe = true;
        }
        if self.awaiting_keyframe && !descriptor.keyframe {
            self.request_keyframe = true;
            return Ok(None);
        }
        let decoded = match self
            .decoder
            .as_mut()
            .expect("decoder exists for the current codec epoch")
            .decode(annex_b)
        {
            Ok(decoded) => decoded,
            Err(error) => {
                self.awaiting_keyframe = true;
                self.request_keyframe = true;
                return Err(error.to_string());
            }
        };
        let Some(image) = decoded else {
            if descriptor.keyframe {
                self.request_keyframe = true;
            }
            return Ok(None);
        };
        let (width, height) = image.dimensions();
        if width != descriptor.width_px as usize || height != descriptor.height_px as usize {
            self.awaiting_keyframe = true;
            self.request_keyframe = true;
            return Err(format!(
                "OpenH264 output {width}x{height} differs from descriptor {}x{}",
                descriptor.width_px, descriptor.height_px
            ));
        }
        let mut bgra = vec![0; width * height * 4];
        image.write_rgba8(&mut bgra);
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        self.awaiting_keyframe = false;
        Ok(Some(NativeFrame {
            width_px: descriptor.width_px,
            height_px: descriptor.height_px,
            data: NativeFrameData::CpuBgra(bgra),
            geometry_epoch: descriptor.geometry_epoch,
            sequence: descriptor.sequence,
            received_at: std::time::Instant::now(),
        }))
    }

    pub(super) fn take_keyframe_request(&mut self) -> bool {
        std::mem::take(&mut self.request_keyframe)
    }
}

#[cfg(test)]
mod tests {
    use cua_media_protocol::{FrameSequence, GeometryEpoch, WindowSessionId};
    use openh264::encoder::Encoder;
    use openh264::formats::{RgbSliceU8, YUVBuffer};

    use super::*;

    #[test]
    fn decodes_an_openh264_annex_b_access_unit() {
        let width = 64usize;
        let height = 64usize;
        let rgb = vec![0x80; width * height * 3];
        let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(&rgb, (width, height)));
        let mut encoder = Encoder::new().unwrap();
        let annex_b = encoder.encode(&yuv).unwrap().to_vec();
        let descriptor = VideoFrameDescriptor {
            session_id: WindowSessionId("windows-h264-roundtrip".into()),
            sequence: FrameSequence(1),
            geometry_epoch: GeometryEpoch(1),
            codec_epoch: CodecEpoch(1),
            width_px: width as u32,
            height_px: height as u32,
            capture_timestamp_us: 1,
            encode_duration_us: None,
            codec: VideoCodec::H264,
            keyframe: true,
        };

        let frame = H264Decoder::new()
            .decode(descriptor, &annex_b)
            .unwrap()
            .expect("the first IDR should decode immediately");
        assert_eq!((frame.width_px, frame.height_px), (64, 64));
        let NativeFrameData::CpuBgra(bgra) = frame.data;
        assert_eq!(bgra.len(), width * height * 4);
    }

    #[test]
    fn waits_for_a_keyframe_after_a_codec_epoch_change() {
        let descriptor = VideoFrameDescriptor {
            session_id: WindowSessionId("windows-h264-recovery".into()),
            sequence: FrameSequence(1),
            geometry_epoch: GeometryEpoch(1),
            codec_epoch: CodecEpoch(2),
            width_px: 64,
            height_px: 64,
            capture_timestamp_us: 1,
            encode_duration_us: None,
            codec: VideoCodec::H264,
            keyframe: false,
        };
        let mut decoder = H264Decoder::new();

        assert!(decoder.decode(descriptor, &[]).unwrap().is_none());
        assert!(decoder.take_keyframe_request());
        assert!(!decoder.take_keyframe_request());
    }
}
