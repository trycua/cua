// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Audio seam for media sessions (MEDIA.md §12).
//!
//! The media plane does not capture or encode audio itself. An
//! [`AudioProvider`] (implemented by `cua-media-codec` on top of PipeWire /
//! ScreenCaptureKit / WASAPI capture and libopus) delivers ready-to-send
//! packets: one Opus packet, or one block of s16le PCM, with its media-clock
//! timestamp. The session numbers them per track, frames them as `RAU2`
//! packets and fans them out with audio priority over video.

use std::sync::Arc;

use cua_media_protocol::v2::{AudioCodecName, AudioTrackStateKind};
use cua_spacesd_provider_api::ProviderError;

/// Kind of capturable source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSourceKind {
    Desktop,
    Application,
}

impl AudioSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Application => "application",
        }
    }
}

/// A capturable audio source as reported by the provider.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSourceInfo {
    /// Stable id; "desktop" always names the desktop mix.
    pub source_id: String,
    pub kind: AudioSourceKind,
    pub name: String,
    /// Owning process for application sources.
    pub pid: Option<u32>,
    pub available: bool,
    pub limitation: Option<String>,
    /// Capturing this source actually captures the desktop mix.
    pub desktop_fallback: bool,
}

/// What the session asks the provider to produce for one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrackConfig {
    pub source_id: String,
    pub codec: AudioCodecName,
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub frame_ms: u16,
    pub bitrate_kbps: u32,
    pub fec: bool,
    pub dtx: bool,
    pub expected_loss_percent: u8,
}

impl AudioTrackConfig {
    /// Samples per channel in one frame.
    pub fn frame_samples(&self) -> u16 {
        (u64::from(self.sample_rate_hz) * u64::from(self.frame_ms) / 1000) as u16
    }
}

/// One encoded audio frame from the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    /// Media-clock time of the first sample (`media::clock`).
    pub pts_us: u64,
    /// Samples per channel.
    pub frame_samples: u16,
    /// Opus packet or interleaved s16le PCM.
    pub payload: Vec<u8>,
    /// DTX / comfort-noise frame after silence.
    pub dtx: bool,
}

/// Receives frames and state changes for one track.
pub trait AudioFrameSink: Send + Sync + 'static {
    fn on_frame(&self, frame: AudioFrame);

    fn on_state(&self, state: AudioTrackStateKind, reason: Option<String>);
}

/// A running track capture.
pub trait AudioTrackLease: Send + Sync + 'static {
    fn stop(&self);

    /// Apply new encoding settings live (bitrate, FEC, DTX, loss). A frame
    /// size change may restart the track; the session then sends a new
    /// `audio_config` with a new epoch.
    fn reconfigure(&self, _config: &AudioTrackConfig) -> Result<(), ProviderError> {
        Err(ProviderError::new(
            cua_spacesd_provider_api::ProviderErrorCode::Unsupported,
            "this audio source cannot change its encoding live",
        ))
    }

    fn set_paused(&self, _paused: bool) {}

    /// Opus decoder pre-skip in samples (0 for PCM).
    fn opus_pre_skip(&self) -> u16 {
        0
    }
}

/// Client microphone into a guest virtual source.
pub trait AudioUplinkSink: Send + Sync + 'static {
    /// One uplink frame, already de-jittered by the session.
    fn on_frame(&self, frame: AudioFrame);

    fn set_muted(&self, _muted: bool) {}

    fn close(&self) {}
}

/// Capture/encode backend.
pub trait AudioProvider: Send + Sync + 'static {
    fn sources(&self) -> Vec<AudioSourceInfo>;

    /// Codecs in preference order.
    fn codecs(&self) -> Vec<AudioCodecName>;

    fn start(
        &self,
        config: &AudioTrackConfig,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Arc<dyn AudioTrackLease>, ProviderError>;

    /// Open the uplink virtual source. `None` when unsupported.
    fn open_uplink(
        &self,
        _config: &AudioTrackConfig,
        _virtual_source_name: &str,
    ) -> Result<Option<Arc<dyn AudioUplinkSink>>, ProviderError> {
        Ok(None)
    }

    /// The source paired with a window (its owning app), falling back to the
    /// desktop mix.
    fn source_for_pid(&self, pid: Option<u32>) -> Option<AudioSourceInfo> {
        let sources = self.sources();
        pid.and_then(|pid| {
            sources
                .iter()
                .find(|source| source.pid == Some(pid) && source.available)
                .cloned()
        })
        .or_else(|| {
            sources
                .into_iter()
                .find(|source| source.kind == AudioSourceKind::Desktop)
        })
    }
}

/// Validate and normalise a requested encoding against the contract limits.
pub fn normalise_config(mut config: AudioTrackConfig) -> AudioTrackConfig {
    if !matches!(
        config.sample_rate_hz,
        8_000 | 12_000 | 16_000 | 24_000 | 48_000
    ) {
        config.sample_rate_hz = 48_000;
    }
    config.channels = config.channels.clamp(1, 2);
    if !matches!(config.frame_ms, 10 | 20 | 40 | 60) {
        config.frame_ms = 20;
    }
    if config.codec == AudioCodecName::Opus {
        if config.bitrate_kbps == 0 {
            config.bitrate_kbps = if config.channels == 2 { 64 } else { 32 };
        }
        config.bitrate_kbps = config.bitrate_kbps.clamp(6, 510);
    } else {
        config.bitrate_kbps = 0;
        config.fec = false;
        config.dtx = false;
    }
    config.expected_loss_percent = config.expected_loss_percent.min(100);
    config
}

/// Largest Opus bitrate whose packets fit one QUIC datagram (MEDIA.md §12.6):
/// `bitrate_kbps * frame_ms <= ~9000`.
pub fn quic_bitrate_cap(frame_ms: u16) -> u32 {
    (9_000 / u32::from(frame_ms.max(1))).min(510)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AudioTrackConfig {
        AudioTrackConfig {
            source_id: "desktop".into(),
            codec: AudioCodecName::Opus,
            sample_rate_hz: 44_100,
            channels: 5,
            frame_ms: 15,
            bitrate_kbps: 0,
            fec: true,
            dtx: true,
            expected_loss_percent: 200,
        }
    }

    #[test]
    fn normalises_to_contract_defaults() {
        let normalised = normalise_config(config());
        assert_eq!(normalised.sample_rate_hz, 48_000);
        assert_eq!(normalised.channels, 2);
        assert_eq!(normalised.frame_ms, 20);
        assert_eq!(normalised.bitrate_kbps, 64);
        assert_eq!(normalised.expected_loss_percent, 100);
        assert_eq!(normalised.frame_samples(), 960);
        let pcm = normalise_config(AudioTrackConfig {
            codec: AudioCodecName::PcmS16le,
            ..config()
        });
        assert_eq!(pcm.bitrate_kbps, 0);
        assert!(!pcm.fec && !pcm.dtx);
    }

    #[test]
    fn quic_cap_matches_media_md_examples() {
        assert!(quic_bitrate_cap(10) >= 510 || quic_bitrate_cap(10) == 510);
        assert!(quic_bitrate_cap(60) >= 128);
        assert_eq!(quic_bitrate_cap(20), 450);
    }
}
