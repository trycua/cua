// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Conversions between the `cua.env.v1` audio contract (`cua-proto`) and
//! this crate's codec types (feature `proto`).
//!
//! Defaults for zero/unset fields follow the contract comments in
//! `stream.proto` (`AudioEncoding`): 48 kHz, 2 channels downlink / 1 uplink,
//! 64/32 kbit/s, FEC and DTX on, 20 ms frames, 10 % expected loss.

use cua_proto::env::v1 as pb;

use crate::audio::codec::{AudioCodecKind, AudioEncodingConfig, OpusApplication};
use crate::audio::{AudioFormat, AudioSourceInfo, AudioSourceKind};
use crate::error::{CodecError, Result};

impl From<AudioCodecKind> for pb::AudioCodec {
    fn from(c: AudioCodecKind) -> Self {
        match c {
            AudioCodecKind::Opus => pb::AudioCodec::Opus,
            AudioCodecKind::PcmS16le => pb::AudioCodec::PcmS16le,
        }
    }
}

impl TryFrom<pb::AudioCodec> for AudioCodecKind {
    type Error = CodecError;
    fn try_from(c: pb::AudioCodec) -> Result<Self> {
        match c {
            pb::AudioCodec::Opus => Ok(Self::Opus),
            pb::AudioCodec::PcmS16le => Ok(Self::PcmS16le),
            pb::AudioCodec::Unspecified => Err(CodecError::InvalidArgument(
                "unspecified audio codec".into(),
            )),
        }
    }
}

impl From<AudioSourceKind> for pb::AudioSourceKind {
    fn from(k: AudioSourceKind) -> Self {
        match k {
            AudioSourceKind::Desktop => pb::AudioSourceKind::Desktop,
            AudioSourceKind::Application => pb::AudioSourceKind::Application,
        }
    }
}

impl From<&AudioSourceInfo> for pb::AudioSource {
    fn from(s: &AudioSourceInfo) -> Self {
        pb::AudioSource {
            source_id: s.source_id.clone(),
            kind: pb::AudioSourceKind::from(s.kind) as i32,
            name: s.name.clone(),
            available: s.available,
            limitation: s.limitation.clone().unwrap_or_default(),
            desktop_fallback: s.desktop_fallback,
            ..Default::default()
        }
    }
}

/// Direction of a track (selects contract defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Guest -> client.
    Downlink,
    /// Client -> guest (microphone).
    Uplink,
}

/// Resolves a requested `AudioEncoding` against `supported` codecs
/// (driver preference order), applying the contract defaults.
pub fn negotiate(
    request: Option<&pb::AudioEncoding>,
    direction: Direction,
    supported: &[AudioCodecKind],
) -> Result<AudioEncodingConfig> {
    let defaults = match direction {
        Direction::Downlink => AudioEncodingConfig::downlink(),
        Direction::Uplink => AudioEncodingConfig::uplink(),
    };
    let Some(req) = request else {
        return Ok(defaults);
    };
    let wanted: Vec<AudioCodecKind> = if req.codecs.is_empty() {
        vec![AudioCodecKind::Opus, AudioCodecKind::PcmS16le]
    } else {
        req.codecs
            .iter()
            .filter_map(|c| pb::AudioCodec::try_from(*c).ok())
            .filter_map(|c| AudioCodecKind::try_from(c).ok())
            .collect()
    };
    let codec = wanted
        .into_iter()
        .find(|c| supported.contains(c))
        .ok_or_else(|| CodecError::Unsupported("no requested audio codec is supported".into()))?;
    let channels = match req.channels {
        0 => defaults.format.channels,
        1 | 2 => req.channels as u16,
        n => {
            return Err(CodecError::InvalidArgument(format!(
                "{n} channels (1 or 2 supported)"
            )))
        }
    };
    let bitrate_kbps = match req.bitrate_kbps {
        0 if channels == 1 => 32,
        0 => 64,
        n => n,
    };
    let fec = req.fec.unwrap_or(true);
    let config = AudioEncodingConfig {
        codec,
        format: AudioFormat {
            sample_rate: if req.sample_rate_hz == 0 {
                48_000
            } else {
                req.sample_rate_hz
            },
            channels,
        },
        bitrate_kbps,
        fec,
        dtx: req.dtx.unwrap_or(true),
        frame_us: u64::from(if req.frame_ms == 0 { 20 } else { req.frame_ms }) * 1000,
        expected_loss_percent: match req.expected_loss_percent {
            0 if fec => 10,
            n => n.min(100) as u8,
        },
        application: OpusApplication::Auto,
        complexity: defaults.complexity,
    };
    let config = if codec == AudioCodecKind::PcmS16le {
        crate::audio::codec::PcmCodec::new(config).config_clone()
    } else {
        config
    };
    config.validate()?;
    Ok(config)
}

impl From<&AudioEncodingConfig> for pb::NegotiatedAudioEncoding {
    fn from(c: &AudioEncodingConfig) -> Self {
        pb::NegotiatedAudioEncoding {
            codec: pb::AudioCodec::from(c.codec) as i32,
            sample_rate_hz: c.format.sample_rate,
            channels: u32::from(c.format.channels),
            bitrate_kbps: if c.codec == AudioCodecKind::PcmS16le {
                0
            } else {
                c.bitrate_kbps
            },
            fec: c.fec,
            dtx: c.dtx,
            frame_ms: c.frame_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_request_uses_contract_defaults() {
        let c = negotiate(
            Some(&pb::AudioEncoding::default()),
            Direction::Downlink,
            &[AudioCodecKind::Opus],
        )
        .unwrap();
        assert_eq!(c, AudioEncodingConfig::downlink());
        let u = negotiate(None, Direction::Uplink, &[AudioCodecKind::Opus]).unwrap();
        assert_eq!((u.format.channels, u.bitrate_kbps), (1, 32));
    }

    #[test]
    fn explicit_request_round_trips_to_negotiated() {
        let req = pb::AudioEncoding {
            codecs: vec![pb::AudioCodec::Opus as i32],
            sample_rate_hz: 48_000,
            channels: 1,
            bitrate_kbps: 24,
            fec: Some(false),
            dtx: Some(false),
            frame_ms: 10,
            expected_loss_percent: 0,
        };
        let c = negotiate(
            Some(&req),
            Direction::Downlink,
            &[AudioCodecKind::Opus, AudioCodecKind::PcmS16le],
        )
        .unwrap();
        assert_eq!(
            (c.frame_us, c.fec, c.dtx, c.expected_loss_percent),
            (10_000, false, false, 0)
        );
        let n = pb::NegotiatedAudioEncoding::from(&c);
        assert_eq!(
            (n.codec, n.channels, n.bitrate_kbps, n.frame_ms),
            (pb::AudioCodec::Opus as i32, 1, 24, 10)
        );
    }

    #[test]
    fn pcm_fallback_and_rejections() {
        let req = pb::AudioEncoding {
            codecs: vec![pb::AudioCodec::PcmS16le as i32],
            ..Default::default()
        };
        let c = negotiate(
            Some(&req),
            Direction::Downlink,
            &[AudioCodecKind::Opus, AudioCodecKind::PcmS16le],
        )
        .unwrap();
        assert_eq!(c.codec, AudioCodecKind::PcmS16le);
        assert_eq!(pb::NegotiatedAudioEncoding::from(&c).bitrate_kbps, 0);
        assert!(negotiate(Some(&req), Direction::Downlink, &[AudioCodecKind::Opus]).is_err());
        let bad = pb::AudioEncoding {
            frame_ms: 15,
            ..Default::default()
        };
        assert!(negotiate(Some(&bad), Direction::Downlink, &[AudioCodecKind::Opus]).is_err());
    }

    #[test]
    fn source_maps_to_proto() {
        let mut s = AudioSourceInfo::desktop("Desktop audio");
        s.desktop_fallback = true;
        s.limitation = Some("x".into());
        let p = pb::AudioSource::from(&s);
        assert_eq!(
            (p.source_id.as_str(), p.kind, p.desktop_fallback),
            ("desktop", pb::AudioSourceKind::Desktop as i32, true)
        );
    }
}
