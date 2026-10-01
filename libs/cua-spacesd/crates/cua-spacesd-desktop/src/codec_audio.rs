// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The media plane's `AudioProvider` over `cua-media-codec` capture and codecs.
//!
//! Capture (PulseAudio/PipeWire, ScreenCaptureKit, WASAPI) delivers s16
//! buffers; a `FrameChunker` cuts them into exact codec frames with
//! contiguous media-clock timestamps; the codec layer encodes Opus (or passes
//! PCM through). DTX silence frames are not sent (their `pts_us` gap tells the
//! client it was silence). Uplink audio is decoded and written into the guest
//! virtual microphone where the platform has one (Linux).

use std::sync::{Arc, Mutex};

use cua_media_codec::audio::codec::{
    open_audio_decoder, open_audio_encoder, AudioCodecKind, AudioDecoder, AudioEncoder,
    AudioEncodingConfig,
};
use cua_media_codec::audio::{
    platform_capture, AudioCapture, AudioCaptureRequest, AudioCaptureState, AudioFormat,
    AudioFrame as CodecFrame, AudioFrameSink as CodecSink, AudioStream, FrameChunker,
};
use cua_media_protocol::v2::{AudioCodecName, AudioTrackStateKind};
use cua_spacesd_provider_api::{ProviderError, ProviderErrorCode};
use cua_spacesd_session::media::audio::{
    AudioFrame, AudioFrameSink, AudioProvider, AudioSourceInfo, AudioSourceKind, AudioTrackConfig,
    AudioTrackLease, AudioUplinkSink,
};

fn codec_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(ProviderErrorCode::Internal, format!("audio: {error}"))
}

fn encoding(config: &AudioTrackConfig) -> AudioEncodingConfig {
    AudioEncodingConfig {
        codec: match config.codec {
            AudioCodecName::PcmS16le => AudioCodecKind::PcmS16le,
            _ => AudioCodecKind::Opus,
        },
        format: AudioFormat {
            sample_rate: config.sample_rate_hz,
            channels: u16::from(config.channels),
        },
        bitrate_kbps: config.bitrate_kbps.max(6),
        fec: config.fec,
        dtx: config.dtx,
        frame_us: u64::from(config.frame_ms) * 1_000,
        expected_loss_percent: config.expected_loss_percent,
        ..AudioEncodingConfig::downlink()
    }
}

/// Audio over the platform's capture backend.
pub struct CodecAudioProvider {
    capture: Box<dyn AudioCapture>,
    opus: bool,
}

impl CodecAudioProvider {
    /// `None` when this machine has no audio capture backend (no sound
    /// server in the container, for example).
    pub fn new() -> Option<Self> {
        let capture = platform_capture().ok()?;
        capture.sources().ok()?;
        let opus = open_audio_encoder(&AudioEncodingConfig::downlink()).is_ok();
        Some(Self { capture, opus })
    }

    /// Wrap an explicit capture backend (tests use the fake).
    pub fn with_capture(capture: Box<dyn AudioCapture>) -> Self {
        let opus = open_audio_encoder(&AudioEncodingConfig::downlink()).is_ok();
        Self { capture, opus }
    }
}

struct TrackPipeline {
    chunker: FrameChunker,
    encoder: Box<dyn AudioEncoder>,
    sink: Arc<dyn AudioFrameSink>,
    paused: bool,
}

struct TrackSink(Mutex<TrackPipeline>);

impl CodecSink for TrackSink {
    fn on_audio(&self, frame: CodecFrame) {
        let mut pipeline = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pipeline.paused {
            return;
        }
        for chunk in pipeline.chunker.push(&frame) {
            let samples = (chunk.samples.len() / usize::from(chunk.format.channels.max(1))) as u16;
            match pipeline.encoder.encode(&chunk.samples) {
                Ok(encoded) if encoded.dtx_silence => {}
                Ok(encoded) => pipeline.sink.on_frame(AudioFrame {
                    pts_us: chunk.pts_us,
                    frame_samples: samples,
                    payload: encoded.data,
                    dtx: false,
                }),
                Err(error) => {
                    tracing::warn!(target: "cua_spacesd_client::audio", %error, "audio encode failed");
                }
            }
        }
    }

    fn on_state(&self, state: AudioCaptureState) {
        let sink = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sink
            .clone();
        let (kind, reason) = match state {
            AudioCaptureState::Active => (AudioTrackStateKind::Active, None),
            AudioCaptureState::Suspended(reason) => (AudioTrackStateKind::Suspended, Some(reason)),
            AudioCaptureState::Fallback(reason) => (AudioTrackStateKind::Fallback, Some(reason)),
            AudioCaptureState::Ended(reason) => (AudioTrackStateKind::Ended, Some(reason)),
        };
        sink.on_state(kind, reason);
    }
}

struct TrackLease {
    stream: Mutex<Option<Box<dyn AudioStream>>>,
    pipeline: Arc<TrackSink>,
    pre_skip: u16,
}

impl AudioTrackLease for TrackLease {
    fn stop(&self) {
        if let Some(mut stream) = self.stream.lock().unwrap().take() {
            stream.stop();
        }
    }

    fn reconfigure(&self, config: &AudioTrackConfig) -> Result<(), ProviderError> {
        let mut pipeline = self.pipeline.0.lock().unwrap();
        let wanted = encoding(config);
        let current = pipeline.encoder.config().clone();
        if wanted.frame_us != current.frame_us
            || wanted.codec != current.codec
            || wanted.format != current.format
        {
            pipeline.encoder = open_audio_encoder(&wanted).map_err(codec_error)?;
            pipeline.chunker = FrameChunker::new(wanted.format, wanted.frame_us);
            return Ok(());
        }
        if wanted.codec == AudioCodecKind::Opus {
            pipeline
                .encoder
                .set_bitrate_kbps(wanted.bitrate_kbps)
                .map_err(codec_error)?;
            pipeline
                .encoder
                .set_fec(wanted.fec, wanted.expected_loss_percent)
                .map_err(codec_error)?;
            pipeline.encoder.set_dtx(wanted.dtx).map_err(codec_error)?;
        }
        Ok(())
    }

    fn set_paused(&self, paused: bool) {
        self.pipeline.0.lock().unwrap().paused = paused;
    }

    fn opus_pre_skip(&self) -> u16 {
        self.pre_skip
    }
}

impl Drop for TrackLease {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Uplink {
    decoder: Mutex<Box<dyn AudioDecoder>>,
    #[cfg(target_os = "linux")]
    output: Mutex<cua_media_codec::audio::pulse::PulseUplink>,
    muted: std::sync::atomic::AtomicBool,
}

impl AudioUplinkSink for Uplink {
    fn on_frame(&self, frame: AudioFrame) {
        if self.muted.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Ok(samples) = self.decoder.lock().unwrap().decode(&frame.payload) else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            use cua_media_codec::audio::AudioSink as _;
            let _ = self.output.lock().unwrap().write(&samples);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = samples;
    }

    fn set_muted(&self, muted: bool) {
        self.muted
            .store(muted, std::sync::atomic::Ordering::Relaxed);
    }
}

impl AudioProvider for CodecAudioProvider {
    fn sources(&self) -> Vec<AudioSourceInfo> {
        self.capture
            .sources()
            .unwrap_or_default()
            .into_iter()
            .map(|source| AudioSourceInfo {
                source_id: source.source_id,
                kind: match source.kind {
                    cua_media_codec::audio::AudioSourceKind::Desktop => AudioSourceKind::Desktop,
                    cua_media_codec::audio::AudioSourceKind::Application => {
                        AudioSourceKind::Application
                    }
                },
                name: source.name,
                pid: source.pid,
                available: source.available,
                limitation: source.limitation,
                desktop_fallback: source.desktop_fallback,
            })
            .collect()
    }

    fn codecs(&self) -> Vec<AudioCodecName> {
        if self.opus {
            vec![AudioCodecName::Opus, AudioCodecName::PcmS16le]
        } else {
            vec![AudioCodecName::PcmS16le]
        }
    }

    fn start(
        &self,
        config: &AudioTrackConfig,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Arc<dyn AudioTrackLease>, ProviderError> {
        let encoding = encoding(config);
        let encoder = open_audio_encoder(&encoding).map_err(codec_error)?;
        let pre_skip = u16::try_from(encoder.pre_skip()).unwrap_or(u16::MAX);
        let pipeline = Arc::new(TrackSink(Mutex::new(TrackPipeline {
            chunker: FrameChunker::new(encoding.format, encoding.frame_us),
            encoder,
            sink,
            paused: false,
        })));
        let request = AudioCaptureRequest {
            source_id: config.source_id.clone(),
            format: encoding.format,
            buffer_ms: 10,
        };
        let stream = self
            .capture
            .start(&request, pipeline.clone())
            .map_err(codec_error)?;
        Ok(Arc::new(TrackLease {
            stream: Mutex::new(Some(stream)),
            pipeline,
            pre_skip,
        }))
    }

    fn open_uplink(
        &self,
        config: &AudioTrackConfig,
        virtual_source_name: &str,
    ) -> Result<Option<Arc<dyn AudioUplinkSink>>, ProviderError> {
        #[cfg(target_os = "linux")]
        {
            let encoding = encoding(config);
            let decoder = open_audio_decoder(&encoding).map_err(codec_error)?;
            let output = cua_media_codec::audio::pulse::PulseUplink::open(
                virtual_source_name,
                encoding.format,
                false,
            )
            .map_err(codec_error)?;
            Ok(Some(Arc::new(Uplink {
                decoder: Mutex::new(decoder),
                output: Mutex::new(output),
                muted: std::sync::atomic::AtomicBool::new(false),
            })))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (config, virtual_source_name, open_audio_decoder);
            Ok(None)
        }
    }
}
