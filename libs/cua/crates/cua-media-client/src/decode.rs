// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Client-side decoding (feature `media-decode`): H.264 access units to
//! packed BGRA (VideoToolbox on macOS, OpenH264 elsewhere) and RAU2 audio
//! packets to interleaved s16 PCM (Opus / PCM), both through
//! `cua-media-codec`.
//!
//! The decoders keep the wire invariants a host would otherwise have to
//! re-implement: a codec-epoch change resets the video decoder, a sequence
//! gap or decode error waits for the next keyframe (and tells the caller to
//! ask for one, again with backoff until one arrives, since a single lost
//! request would otherwise freeze the picture), and short audio gaps are
//! concealed so playback stays on the media clock.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cua_media_codec::audio::codec::{
    open_audio_decoder, AudioCodecKind, AudioDecoder, AudioEncodingConfig,
};
use cua_media_codec::audio::AudioFormat;
use cua_media_protocol::v2::{AudioCodecName, AudioConfig, AudioDirection};

/// One decoded frame, packed top-down BGRA (`stride = width * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedVideo {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub capture_timestamp_us: u64,
    pub codec_epoch: u64,
    pub geometry_epoch: u64,
    /// The source access unit was a keyframe.
    pub keyframe: bool,
    /// Bytes of the source access unit.
    pub encoded_size: u64,
    pub bgra: Vec<u8>,
}

/// First keyframe re-request interval; doubles up to [`KEYFRAME_RETRY_MAX`].
pub const KEYFRAME_RETRY_MIN: Duration = Duration::from_millis(500);
/// Longest keyframe re-request interval.
pub const KEYFRAME_RETRY_MAX: Duration = Duration::from_secs(2);

/// One encoded frame as it arrives.
#[derive(Debug, Clone, Copy)]
pub struct EncodedVideo<'a> {
    /// `h264`, `bgra` or `png`.
    pub codec: &'a str,
    pub keyframe: bool,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub capture_timestamp_us: u64,
    pub codec_epoch: u64,
    pub geometry_epoch: u64,
    pub data: &'a [u8],
}

/// Result of feeding one encoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoOutcome {
    Frame(DecodedVideo),
    /// Nothing to show yet (decoder buffering, or dependent frames dropped
    /// while waiting for a keyframe).
    Pending,
    /// References were lost: request a keyframe. Frames are dropped until
    /// one arrives; while none does, this repeats with backoff
    /// ([`KEYFRAME_RETRY_MIN`] doubling to [`KEYFRAME_RETRY_MAX`]).
    NeedKeyframe,
}

/// Stateful video decoder for one media session.
#[derive(Default)]
pub struct VideoDecoder {
    h264: Option<Box<dyn cua_media_codec::video::VideoDecoder>>,
    codec_epoch: Option<u64>,
    last_sequence: Option<u64>,
    waiting_for_keyframe: bool,
    /// When a keyframe was last asked for while waiting, and the interval
    /// before asking again.
    last_request: Option<Instant>,
    retry: Duration,
}

impl VideoDecoder {
    pub fn new() -> Self {
        Self {
            waiting_for_keyframe: true,
            retry: KEYFRAME_RETRY_MIN,
            ..Self::default()
        }
    }

    /// Whether a keyframe should be (re)requested now: the decoder waits
    /// for one and the backoff interval since the last request elapsed.
    /// Callers also poll this on a timer, so a stream whose frames stopped
    /// entirely still recovers.
    pub fn poll_keyframe_request(&mut self, now: Instant) -> bool {
        if !self.waiting_for_keyframe {
            return false;
        }
        let due = self
            .last_request
            .is_none_or(|at| now.saturating_duration_since(at) >= self.retry);
        if due {
            if self.last_request.is_some() {
                self.retry = (self.retry * 2).min(KEYFRAME_RETRY_MAX);
            }
            self.last_request = Some(now);
        }
        due
    }

    fn keyframe_arrived(&mut self) {
        self.waiting_for_keyframe = false;
        self.last_request = None;
        self.retry = KEYFRAME_RETRY_MIN;
    }

    /// Name of the H.264 backend once one is open.
    pub fn backend(&self) -> Option<String> {
        self.h264
            .as_ref()
            .map(|decoder| format!("{:?}", decoder.backend()).to_lowercase())
    }

    /// Whether the open H.264 decoder runs in hardware (`None` until a
    /// keyframe opened one, or when the backend cannot tell).
    pub fn hardware(&self) -> Option<bool> {
        self.h264.as_ref().and_then(|decoder| decoder.is_hardware())
    }

    pub fn decode(&mut self, frame: EncodedVideo<'_>) -> Result<VideoOutcome, String> {
        self.decode_at(frame, Instant::now())
    }

    /// [`Self::decode`] at an explicit time (tests).
    pub fn decode_at(
        &mut self,
        frame: EncodedVideo<'_>,
        now: Instant,
    ) -> Result<VideoOutcome, String> {
        let gap = self
            .last_sequence
            .is_some_and(|last| frame.sequence != last.wrapping_add(1));
        self.last_sequence = Some(frame.sequence);
        match frame.codec {
            "bgra" => {
                let expected = frame.width as usize * frame.height as usize * 4;
                if frame.data.len() != expected {
                    return Err(format!(
                        "BGRA payload is {} bytes, expected {expected}",
                        frame.data.len()
                    ));
                }
                Ok(VideoOutcome::Frame(decoded(
                    &frame,
                    frame.width,
                    frame.height,
                    frame.data.to_vec(),
                )))
            }
            "h264" => self.decode_h264(frame, gap, now),
            other => Err(format!(
                "decoding {other} is not supported; use the encoded callbacks"
            )),
        }
    }

    fn decode_h264(
        &mut self,
        frame: EncodedVideo<'_>,
        gap: bool,
        now: Instant,
    ) -> Result<VideoOutcome, String> {
        if self.codec_epoch != Some(frame.codec_epoch) {
            self.codec_epoch = Some(frame.codec_epoch);
            if let Some(decoder) = self.h264.as_mut() {
                decoder.reset().map_err(|error| error.to_string())?;
            }
            self.waiting_for_keyframe = true;
        }
        if gap && !frame.keyframe && !self.waiting_for_keyframe {
            // Fresh loss: ask right away.
            self.waiting_for_keyframe = true;
            self.last_request = None;
            self.retry = KEYFRAME_RETRY_MIN;
        }
        if self.waiting_for_keyframe {
            if !frame.keyframe {
                return Ok(if self.poll_keyframe_request(now) {
                    VideoOutcome::NeedKeyframe
                } else {
                    VideoOutcome::Pending
                });
            }
            self.keyframe_arrived();
        }
        if self.h264.is_none() {
            self.h264 = Some(
                cua_media_codec::backends::open_best_decoder(
                    cua_media_codec::types::VideoCodec::H264,
                )
                .map_err(|error| error.to_string())?,
            );
        }
        let decoder = self.h264.as_mut().expect("decoder opened above");
        match decoder.decode(frame.data, frame.capture_timestamp_us) {
            Ok(Some(picture)) => {
                let bgra = picture.to_bgra();
                Ok(VideoOutcome::Frame(decoded(
                    &frame,
                    picture.width,
                    picture.height,
                    bgra,
                )))
            }
            Ok(None) => Ok(VideoOutcome::Pending),
            Err(_) => {
                let _ = decoder.reset();
                self.waiting_for_keyframe = true;
                self.last_request = Some(now);
                self.retry = KEYFRAME_RETRY_MIN;
                Ok(VideoOutcome::NeedKeyframe)
            }
        }
    }
}

fn decoded(frame: &EncodedVideo<'_>, width: u32, height: u32, bgra: Vec<u8>) -> DecodedVideo {
    DecodedVideo {
        sequence: frame.sequence,
        width,
        height,
        capture_timestamp_us: frame.capture_timestamp_us,
        codec_epoch: frame.codec_epoch,
        geometry_epoch: frame.geometry_epoch,
        keyframe: frame.keyframe,
        encoded_size: frame.data.len() as u64,
        bgra,
    }
}

/// Decoded audio: interleaved s16 samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame {
    pub track_id: u16,
    pub sample_rate: u32,
    pub channels: u16,
    /// Media-clock time of the first sample (µs).
    pub pts_us: u64,
    /// The frame was synthesized by packet-loss concealment.
    pub concealed: bool,
    pub samples: Vec<i16>,
}

/// The RAU2 fields the decoder needs.
#[derive(Debug, Clone, Copy)]
pub struct EncodedAudio<'a> {
    pub track_id: u16,
    pub sequence: u32,
    pub pts_us: u64,
    pub frame_samples: u16,
    pub config_epoch: u8,
    pub discontinuity: bool,
    pub dtx: bool,
    pub data: &'a [u8],
}

struct Track {
    decoder: Box<dyn AudioDecoder>,
    config_epoch: u8,
    format: AudioFormat,
    last_sequence: Option<u32>,
}

/// Longest gap filled by concealment; longer gaps just resume.
pub const MAX_CONCEALED_FRAMES: u32 = 5;

/// Per-track audio decoders, configured from `audio_config` messages.
#[derive(Default)]
pub struct AudioDecoders {
    tracks: HashMap<u16, Track>,
}

impl AudioDecoders {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an `audio_config` (downlink tracks only).
    pub fn configure(&mut self, config: &AudioConfig) -> Result<(), String> {
        if config.direction != AudioDirection::Down {
            return Ok(());
        }
        let codec = match config.codec {
            AudioCodecName::Opus => AudioCodecKind::Opus,
            AudioCodecName::PcmS16le => AudioCodecKind::PcmS16le,
            AudioCodecName::Unknown => {
                return Err(format!("track {} uses an unknown codec", config.track_id))
            }
        };
        let format = AudioFormat {
            sample_rate: config.sample_rate_hz,
            channels: u16::from(config.channels),
        };
        let decoder = open_audio_decoder(&AudioEncodingConfig {
            codec,
            format,
            bitrate_kbps: config.bitrate_kbps.max(6),
            fec: config.fec,
            dtx: config.dtx,
            frame_us: u64::from(config.frame_ms) * 1_000,
            ..AudioEncodingConfig::default()
        })
        .map_err(|error| error.to_string())?;
        self.tracks.insert(
            config.track_id,
            Track {
                decoder,
                config_epoch: config.config_epoch,
                format,
                last_sequence: None,
            },
        );
        Ok(())
    }

    /// Apply an `audio_config` given as the control message JSON.
    pub fn configure_json(&mut self, json: &str) -> Result<(), String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        let payload = value.get("payload").cloned().unwrap_or(value);
        let config: AudioConfig =
            serde_json::from_value(payload).map_err(|error| error.to_string())?;
        self.configure(&config)
    }

    /// Decode one packet. Unknown tracks and stale config epochs yield
    /// nothing; short gaps are concealed first.
    pub fn decode(&mut self, packet: EncodedAudio<'_>) -> Vec<PcmFrame> {
        let Some(track) = self.tracks.get_mut(&packet.track_id) else {
            return Vec::new();
        };
        if packet.config_epoch != track.config_epoch {
            return Vec::new();
        }
        let mut out = Vec::new();
        let frame_samples = usize::from(packet.frame_samples);
        let frame_us = u64::from(packet.frame_samples) * 1_000_000
            / u64::from(track.format.sample_rate.max(1));
        if let Some(last) = track.last_sequence {
            let gap = packet.sequence.wrapping_sub(last).wrapping_sub(1);
            if gap >= 0x8000_0000 {
                // Older than what was already played.
                return out;
            }
            if !packet.discontinuity && (1..=MAX_CONCEALED_FRAMES).contains(&gap) {
                for index in 0..gap {
                    if let Ok(samples) = track.decoder.conceal(frame_samples) {
                        out.push(PcmFrame {
                            track_id: packet.track_id,
                            sample_rate: track.format.sample_rate,
                            channels: track.format.channels,
                            pts_us: packet
                                .pts_us
                                .saturating_sub(frame_us * u64::from(gap - index)),
                            concealed: true,
                            samples,
                        });
                    }
                }
            }
        }
        track.last_sequence = Some(packet.sequence);
        if packet.dtx || packet.data.is_empty() {
            return out;
        }
        if let Ok(samples) = track.decoder.decode(packet.data) {
            out.push(PcmFrame {
                track_id: packet.track_id,
                sample_rate: track.format.sample_rate,
                channels: track.format.channels,
                pts_us: packet.pts_us,
                concealed: false,
                samples,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_codec::audio::codec::open_audio_encoder;
    use cua_media_codec::types::{EncoderConfig, VideoCodec, VideoFrame};

    fn h264_stream(frames: usize) -> Vec<(bool, Vec<u8>)> {
        let mut encoder = cua_media_codec::backends::open_encoder(
            cua_media_codec::types::Backend::OpenH264,
            &EncoderConfig::new(VideoCodec::H264, 64, 48, 30),
        )
        .unwrap();
        (0..frames)
            .map(|index| {
                let bgra: Vec<u8> = (0..64 * 48)
                    .flat_map(|pixel| [(pixel + index * 7) as u8, 80, 160, 255])
                    .collect();
                let frame = VideoFrame::bgra(64, 48, index as u64 * 33_000, &bgra);
                let unit = encoder.encode(&frame).unwrap().into_iter().next().unwrap();
                (unit.keyframe, unit.data)
            })
            .collect()
    }

    fn encoded<'a>(sequence: u64, keyframe: bool, data: &'a [u8]) -> EncodedVideo<'a> {
        EncodedVideo {
            codec: "h264",
            keyframe,
            sequence,
            width: 64,
            height: 48,
            capture_timestamp_us: sequence * 33_000,
            codec_epoch: 1,
            geometry_epoch: 1,
            data,
        }
    }

    #[test]
    fn h264_decodes_to_bgra_and_a_gap_waits_for_a_keyframe() {
        let stream = h264_stream(4);
        assert!(stream[0].0, "first unit is an IDR");
        let mut decoder = VideoDecoder::new();
        // A delta before any keyframe is held back (and a keyframe asked for).
        assert_eq!(
            decoder.decode(encoded(0, false, &stream[1].1)).unwrap(),
            VideoOutcome::NeedKeyframe
        );
        let VideoOutcome::Frame(frame) = decoder.decode(encoded(1, true, &stream[0].1)).unwrap()
        else {
            panic!("keyframe decodes")
        };
        assert_eq!(
            (frame.width, frame.height, frame.bgra.len()),
            (64, 48, 64 * 48 * 4)
        );
        assert!(matches!(
            decoder.decode(encoded(2, false, &stream[1].1)).unwrap(),
            VideoOutcome::Frame(_)
        ));
        // Sequence 3 lost: ask, then hold (no re-ask within the backoff)
        // until a keyframe.
        assert_eq!(
            decoder.decode(encoded(4, false, &stream[3].1)).unwrap(),
            VideoOutcome::NeedKeyframe
        );
        assert_eq!(
            decoder.decode(encoded(5, false, &stream[3].1)).unwrap(),
            VideoOutcome::Pending
        );
        assert!(matches!(
            decoder.decode(encoded(6, true, &stream[0].1)).unwrap(),
            VideoOutcome::Frame(_)
        ));
        assert!(decoder.backend().is_some());
    }

    #[test]
    fn a_lost_keyframe_request_is_repeated_with_backoff() {
        let stream = h264_stream(3);
        let mut decoder = VideoDecoder::new();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let VideoOutcome::Frame(frame) = decoder
            .decode_at(encoded(0, true, &stream[0].1), at(0))
            .unwrap()
        else {
            panic!("keyframe decodes")
        };
        assert!(frame.keyframe && frame.encoded_size == stream[0].1.len() as u64);
        // Loss: ask now, then only after 500 ms, 1 s, 2 s, 2 s...
        let unit = stream[1].1.clone();
        let delta = |d: &mut VideoDecoder, seq, ms| {
            d.decode_at(encoded(seq, false, &unit), at(ms)).unwrap()
        };
        assert_eq!(delta(&mut decoder, 5, 10), VideoOutcome::NeedKeyframe);
        assert_eq!(delta(&mut decoder, 6, 400), VideoOutcome::Pending);
        assert_eq!(delta(&mut decoder, 7, 520), VideoOutcome::NeedKeyframe);
        assert_eq!(delta(&mut decoder, 8, 1_300), VideoOutcome::Pending);
        assert_eq!(delta(&mut decoder, 9, 1_530), VideoOutcome::NeedKeyframe);
        // No frames at all: the timer path keeps asking (2 s cap).
        assert!(!decoder.poll_keyframe_request(at(3_000)));
        assert!(decoder.poll_keyframe_request(at(3_540)));
        assert!(!decoder.poll_keyframe_request(at(5_000)));
        assert!(decoder.poll_keyframe_request(at(5_550)));
        // A keyframe ends the retries and resets the backoff.
        assert!(matches!(
            decoder
                .decode_at(encoded(10, true, &stream[0].1), at(5_600))
                .unwrap(),
            VideoOutcome::Frame(_)
        ));
        assert!(!decoder.poll_keyframe_request(at(9_000)));
        assert_eq!(delta(&mut decoder, 12, 9_010), VideoOutcome::NeedKeyframe);
        assert_eq!(delta(&mut decoder, 13, 9_520), VideoOutcome::NeedKeyframe);
    }

    #[test]
    fn bgra_passes_through_and_png_is_refused() {
        let mut decoder = VideoDecoder::new();
        let data = vec![1u8; 2 * 2 * 4];
        let frame = EncodedVideo {
            codec: "bgra",
            width: 2,
            height: 2,
            ..encoded(0, true, &data)
        };
        assert!(
            matches!(decoder.decode(frame).unwrap(), VideoOutcome::Frame(ref f) if f.bgra == data)
        );
        assert!(decoder
            .decode(EncodedVideo {
                codec: "bgra",
                width: 3,
                ..frame
            })
            .is_err());
        assert!(decoder
            .decode(EncodedVideo {
                codec: "png",
                ..frame
            })
            .is_err());
    }

    #[test]
    fn opus_packets_decode_to_pcm_with_concealment() {
        let mut decoders = AudioDecoders::new();
        decoders
            .configure_json(
                r#"{"type":"audio_config","payload":{"track_id":1,"config_epoch":2,"direction":"down","codec":"opus",
                "sample_rate_hz":48000,"channels":2,"frame_ms":20,"bitrate_kbps":64,"fec":false,"dtx":false,
                "opus_pre_skip":312,"source":{"source_id":"desktop","kind":"desktop"}}}"#,
            )
            .unwrap();
        let mut encoder = open_audio_encoder(&AudioEncodingConfig {
            dtx: false,
            ..AudioEncodingConfig::downlink()
        })
        .unwrap();
        let pcm = vec![1000i16; 960 * 2];
        let mut frames = Vec::new();
        for sequence in [10u32, 11, 13] {
            let data = encoder.encode(&pcm).unwrap().data;
            frames.extend(decoders.decode(EncodedAudio {
                track_id: 1,
                sequence,
                pts_us: u64::from(sequence) * 20_000,
                frame_samples: 960,
                config_epoch: 2,
                discontinuity: false,
                dtx: false,
                data: &data,
            }));
        }
        assert_eq!(frames.len(), 4);
        assert!(frames[2].concealed && !frames[3].concealed);
        assert_eq!(frames[2].pts_us, 12 * 20_000);
        assert!(frames
            .iter()
            .all(|frame| frame.samples.len() == 960 * 2 && frame.sample_rate == 48_000));
        // Stale epoch and unknown track are ignored.
        let data = encoder.encode(&pcm).unwrap().data;
        let stale = EncodedAudio {
            track_id: 1,
            sequence: 14,
            pts_us: 0,
            frame_samples: 960,
            config_epoch: 1,
            discontinuity: false,
            dtx: false,
            data: &data,
        };
        assert!(decoders.decode(stale).is_empty());
        assert!(decoders
            .decode(EncodedAudio {
                track_id: 9,
                config_epoch: 2,
                ..stale
            })
            .is_empty());
    }
}
