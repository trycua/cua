// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Encoder/decoder negotiation and automatic runtime fallback.
//!
//! The client lists the decoders it has; the server intersects them with
//! the usable encoders from [`crate::probe`] and ranks every (encoder,
//! codec) pair by latency class, backend priority (fastest first), codec
//! preference, and whether the client can decode it in hardware. The
//! ranked list doubles as the fallback chain: [`FallbackEncoder`] moves to
//! the next pair when a backend fails at runtime, starting a new codec epoch
//! with a keyframe.

use serde::{Deserialize, Serialize};

use crate::backends::{DefaultFactory, EncoderFactory};
use crate::error::{CodecError, Result};
use crate::probe::EncoderInfo;
use crate::types::{
    AccessUnit, Backend, EncoderConfig, LatencyClass, Reconfigured, VideoCodec, VideoFrame,
};
use crate::video::VideoEncoder;

/// Decoder families a client may report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoderKind {
    /// Apple VideoToolbox.
    VideoToolbox,
    /// Browser WebCodecs `VideoDecoder`.
    WebCodecs,
    /// NVIDIA NVDEC.
    Nvdec,
    /// VA-API.
    Vaapi,
    /// Windows Media Foundation / D3D11VA.
    MediaFoundation,
    /// OpenH264 software.
    OpenH264,
    /// dav1d software.
    Dav1d,
}

/// One client decoder capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientDecoder {
    /// Decoder family.
    pub kind: DecoderKind,
    /// Codecs it decodes.
    pub codecs: Vec<VideoCodec>,
    /// Hardware-accelerated.
    pub hardware: bool,
}

impl ClientDecoder {
    /// Convenience constructor.
    pub fn new(kind: DecoderKind, codecs: &[VideoCodec], hardware: bool) -> Self {
        Self {
            kind,
            codecs: codecs.to_vec(),
            hardware,
        }
    }
}

/// Server/operator preferences.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SelectionPrefs {
    /// Codec order (earlier is preferred). Empty = H.264, HEVC, AV1.
    pub codec_order: Vec<VideoCodec>,
    /// Pin a backend. If it cannot serve any client codec, selection falls
    /// back to the automatic order (it never fails because of the pin).
    pub pinned_backend: Option<Backend>,
    /// Refuse software encoders.
    pub require_hardware: bool,
    /// Worst latency class acceptable (default: `SoftwareRealtime`).
    pub max_latency_class: Option<LatencyClass>,
}

impl SelectionPrefs {
    /// Reads `CUA_CODEC_ENCODER=<backend>` and `CUA_CODEC_CODECS=h264,av1`.
    pub fn from_env() -> Self {
        let mut prefs = Self::default();
        if let Ok(b) = std::env::var("CUA_CODEC_ENCODER") {
            prefs.pinned_backend = serde_json::from_value(serde_json::Value::String(b)).ok();
        }
        if let Ok(list) = std::env::var("CUA_CODEC_CODECS") {
            prefs.codec_order = list
                .split(',')
                .filter_map(|c| {
                    serde_json::from_value(serde_json::Value::String(c.trim().to_owned())).ok()
                })
                .collect();
        }
        prefs
    }
}

/// One ranked (encoder, codec, decoder) pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Encoder backend.
    pub backend: Backend,
    /// Codec.
    pub codec: VideoCodec,
    /// Hardware encoder.
    pub hardware: bool,
    /// Latency class of the encoder.
    pub latency_class: LatencyClass,
    /// The client decoder to use.
    pub decoder: DecoderKind,
    /// Whether that decoder is hardware.
    pub decoder_hardware: bool,
}

/// Result of [`select`]: the best candidate plus the fallback chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Ranked candidates, best first. Never empty.
    pub candidates: Vec<Candidate>,
}

impl Choice {
    /// The selected pairing.
    pub fn best(&self) -> &Candidate {
        &self.candidates[0]
    }
}

/// Picks the best encoder/decoder pairing.
pub fn select(
    encoders: &[EncoderInfo],
    client: &[ClientDecoder],
    prefs: &SelectionPrefs,
) -> Result<Choice> {
    let codec_order: Vec<VideoCodec> = if prefs.codec_order.is_empty() {
        VideoCodec::ALL.to_vec()
    } else {
        prefs.codec_order.clone()
    };
    let max_class = prefs
        .max_latency_class
        .unwrap_or(LatencyClass::SoftwareRealtime);
    let mut candidates = Vec::new();
    for enc in encoders.iter().filter(|e| e.is_usable()) {
        if prefs.require_hardware && !enc.hardware {
            continue;
        }
        if enc.latency_class > max_class {
            continue;
        }
        for codec in &enc.codecs {
            if !codec_order.contains(codec) {
                continue;
            }
            // Best client decoder for this codec: hardware first.
            let decoder = client
                .iter()
                .filter(|d| d.codecs.contains(codec))
                .max_by_key(|d| d.hardware);
            if let Some(d) = decoder {
                candidates.push(Candidate {
                    backend: enc.backend,
                    codec: *codec,
                    hardware: enc.hardware,
                    latency_class: enc.latency_class,
                    decoder: d.kind,
                    decoder_hardware: d.hardware,
                });
            }
        }
    }
    if candidates.is_empty() {
        return Err(CodecError::NoEncoder(format!(
            "no usable encoder produces a codec the client decodes (encoders: {:?}; client: {:?})",
            encoders
                .iter()
                .filter(|e| e.is_usable())
                .map(|e| (e.backend, e.codecs.clone()))
                .collect::<Vec<_>>(),
            client
                .iter()
                .map(|d| (d.kind, d.codecs.clone()))
                .collect::<Vec<_>>()
        )));
    }
    let codec_rank = |c: VideoCodec| {
        codec_order
            .iter()
            .position(|x| *x == c)
            .unwrap_or(usize::MAX)
    };
    candidates.sort_by_key(|c| {
        (
            prefs
                .pinned_backend
                .map_or(0, |p| usize::from(p != c.backend)),
            c.latency_class,
            !c.decoder_hardware,
            codec_rank(c.codec),
            c.backend.priority(),
        )
    });
    Ok(Choice { candidates })
}

/// Runtime health knobs for [`FallbackEncoder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackPolicy {
    /// Consecutive failures on one backend before moving on.
    pub max_consecutive_failures: u32,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self {
            max_consecutive_failures: 1,
        }
    }
}

/// An encoder that walks a [`Choice`]'s candidates on runtime errors.
///
/// Codec epochs are monotonic across backends: every switch (and every
/// restart inside a backend) produces a new epoch whose first access unit is
/// a keyframe with parameter sets, so clients reset their decoder exactly
/// when the epoch changes. If the codec changes, the caller learns it from
/// [`AccessUnit::codec`] (and should re-negotiate the client decoder).
pub struct FallbackEncoder {
    factory: Box<dyn EncoderFactory>,
    candidates: Vec<Candidate>,
    index: usize,
    config: EncoderConfig,
    current: Option<Box<dyn VideoEncoder>>,
    epoch_base: u64,
    last_inner_epoch: u64,
    failures: u32,
    policy: FallbackPolicy,
    /// Backends that failed, with the error (for capabilities/limitations).
    pub failed: Vec<(Backend, String)>,
}

impl FallbackEncoder {
    /// Opens the best candidate with the default factory.
    pub fn open(choice: &Choice, config: &EncoderConfig) -> Result<Self> {
        Self::with_factory(
            choice,
            config,
            Box::new(DefaultFactory),
            FallbackPolicy::default(),
        )
    }

    /// Opens with an explicit factory (tests inject failures here).
    pub fn with_factory(
        choice: &Choice,
        config: &EncoderConfig,
        factory: Box<dyn EncoderFactory>,
        policy: FallbackPolicy,
    ) -> Result<Self> {
        let mut me = Self {
            factory,
            candidates: choice.candidates.clone(),
            index: 0,
            config: config.clone(),
            current: None,
            epoch_base: 0,
            last_inner_epoch: 0,
            failures: 0,
            policy,
            failed: Vec::new(),
        };
        me.open_from(0)?;
        Ok(me)
    }

    /// The candidate in use.
    pub fn active(&self) -> &Candidate {
        &self.candidates[self.index]
    }

    fn open_from(&mut self, start: usize) -> Result<()> {
        for i in start..self.candidates.len() {
            let cand = &self.candidates[i];
            let mut cfg = self.config.clone();
            cfg.codec = cand.codec;
            match self.factory.open(cand.backend, &cfg) {
                Ok(enc) => {
                    // Next epoch continues after everything emitted so far.
                    self.epoch_base = self.codec_epoch();
                    self.last_inner_epoch = 0;
                    self.current = Some(enc);
                    self.index = i;
                    self.config = cfg;
                    self.failures = 0;
                    if i != start || start != 0 {
                        tracing::warn!(backend = ?cand.backend, codec = ?cand.codec, "encoder fallback engaged");
                    }
                    return Ok(());
                }
                Err(e) => self.failed.push((cand.backend, e.to_string())),
            }
        }
        Err(CodecError::NoEncoder(format!(
            "all candidates failed: {:?}",
            self.failed
        )))
    }

    fn fail_over(&mut self, error: &CodecError) -> Result<()> {
        let cand = self.candidates[self.index].clone();
        self.failed.push((cand.backend, error.to_string()));
        self.current = None;
        self.open_from(self.index + 1)
    }

    fn map_epoch(&mut self, inner: u64) -> u64 {
        self.last_inner_epoch = inner;
        self.epoch_base + inner
    }
}

impl VideoEncoder for FallbackEncoder {
    fn backend(&self) -> Backend {
        self.active().backend
    }

    fn config(&self) -> &EncoderConfig {
        &self.config
    }

    fn codec_epoch(&self) -> u64 {
        self.epoch_base
            + self.current.as_ref().map_or(self.last_inner_epoch, |c| {
                c.codec_epoch().max(self.last_inner_epoch)
            })
    }

    fn reconfigure(&mut self, config: &EncoderConfig) -> Result<Reconfigured> {
        let mut cfg = config.clone();
        cfg.codec = self.active().codec;
        let result = match self.current.as_mut() {
            Some(enc) => enc.reconfigure(&cfg),
            None => Err(CodecError::NoEncoder("no active encoder".into())),
        };
        match result {
            Ok(Reconfigured::InPlace) => {
                self.config = cfg;
                Ok(Reconfigured::InPlace)
            }
            Ok(Reconfigured::Restarted { codec_epoch }) => {
                self.config = cfg;
                Ok(Reconfigured::Restarted {
                    codec_epoch: self.map_epoch(codec_epoch),
                })
            }
            Err(e) if e.is_backend_failure() => {
                self.config = cfg;
                self.fail_over(&e)?;
                Ok(Reconfigured::Restarted {
                    codec_epoch: self.codec_epoch(),
                })
            }
            Err(e) => Err(e),
        }
    }

    fn force_keyframe(&mut self) {
        if let Some(enc) = self.current.as_mut() {
            enc.force_keyframe();
        }
    }

    fn encode(&mut self, frame: &VideoFrame<'_>) -> Result<Vec<AccessUnit>> {
        loop {
            let enc = self
                .current
                .as_mut()
                .ok_or_else(|| CodecError::NoEncoder("no active encoder".into()))?;
            match enc.encode(frame) {
                Ok(mut units) => {
                    self.failures = 0;
                    for au in &mut units {
                        au.codec_epoch += self.epoch_base;
                        self.last_inner_epoch =
                            self.last_inner_epoch.max(au.codec_epoch - self.epoch_base);
                    }
                    return Ok(units);
                }
                Err(e) if e.is_backend_failure() => {
                    self.failures += 1;
                    if self.failures < self.policy.max_consecutive_failures {
                        return Err(e);
                    }
                    tracing::warn!(error = %e, "encoder failed; falling back");
                    self.fail_over(&e)?;
                    // Retry the same frame on the next backend.
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn flush(&mut self) -> Result<Vec<AccessUnit>> {
        match self.current.as_mut() {
            Some(enc) => enc.flush(),
            None => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::ProbeStatus;

    fn info(backend: Backend, codecs: &[VideoCodec], class: LatencyClass) -> EncoderInfo {
        EncoderInfo {
            backend,
            codecs: codecs.to_vec(),
            hardware: backend.is_hardware(),
            max_width: 4096,
            max_height: 4096,
            latency_class: class,
            limitations: vec![],
            status: ProbeStatus::Available,
            device: None,
            probe_ms: 0,
        }
    }

    use LatencyClass::*;
    use VideoCodec::*;

    #[test]
    fn hardware_beats_software_and_priority_breaks_ties() {
        let encoders = [
            info(Backend::OpenH264, &[H264], SoftwareRealtime),
            info(Backend::VideoToolbox, &[H264, Hevc], Hardware),
            info(Backend::Nvenc, &[H264, Hevc, Av1], Hardware),
        ];
        let client = [ClientDecoder::new(
            DecoderKind::WebCodecs,
            &[H264, Hevc, Av1],
            true,
        )];
        let choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
        assert_eq!(choice.best().backend, Backend::Nvenc);
        assert_eq!(choice.best().codec, H264);
        assert_eq!(choice.candidates.last().unwrap().backend, Backend::OpenH264);
    }

    #[test]
    fn detected_only_backends_are_never_selected() {
        let mut nv = info(Backend::Nvenc, &[H264], Hardware);
        nv.status = ProbeStatus::DetectedOnly;
        let encoders = [nv, info(Backend::OpenH264, &[H264], SoftwareRealtime)];
        let client = [ClientDecoder::new(DecoderKind::OpenH264, &[H264], false)];
        let choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
        assert_eq!(choice.candidates.len(), 1);
        assert_eq!(choice.best().backend, Backend::OpenH264);
    }

    #[test]
    fn client_codec_support_limits_the_choice() {
        let encoders = [
            info(Backend::Nvenc, &[Hevc, Av1], Hardware),
            info(Backend::OpenH264, &[H264], SoftwareRealtime),
        ];
        // Safari-like client without AV1, HEVC in hardware.
        let client = [ClientDecoder::new(
            DecoderKind::WebCodecs,
            &[H264, Hevc],
            true,
        )];
        let choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
        assert_eq!(
            (choice.best().backend, choice.best().codec),
            (Backend::Nvenc, Hevc)
        );
        // A client with only software H.264 gets OpenH264.
        let client = [ClientDecoder::new(DecoderKind::OpenH264, &[H264], false)];
        let choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
        assert_eq!(choice.best().backend, Backend::OpenH264);
    }

    #[test]
    fn hardware_client_decode_is_preferred_over_codec_order() {
        let encoders = [info(Backend::VideoToolbox, &[H264, Hevc], Hardware)];
        let client = [
            ClientDecoder::new(DecoderKind::OpenH264, &[H264], false),
            ClientDecoder::new(DecoderKind::VideoToolbox, &[Hevc], true),
        ];
        let choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
        assert_eq!(choice.best().codec, Hevc);
        assert_eq!(choice.best().decoder, DecoderKind::VideoToolbox);
    }

    #[test]
    fn pin_is_honoured_and_falls_back_when_impossible() {
        let encoders = [
            info(Backend::VideoToolbox, &[H264], Hardware),
            info(Backend::OpenH264, &[H264], SoftwareRealtime),
        ];
        let client = [ClientDecoder::new(DecoderKind::WebCodecs, &[H264], true)];
        let prefs = SelectionPrefs {
            pinned_backend: Some(Backend::OpenH264),
            ..Default::default()
        };
        assert_eq!(
            select(&encoders, &client, &prefs).unwrap().best().backend,
            Backend::OpenH264
        );
        let prefs = SelectionPrefs {
            pinned_backend: Some(Backend::Nvenc),
            ..Default::default()
        };
        assert_eq!(
            select(&encoders, &client, &prefs).unwrap().best().backend,
            Backend::VideoToolbox
        );
    }

    #[test]
    fn slow_software_is_excluded_by_default_and_hardware_can_be_required() {
        let encoders = [
            info(Backend::Rav1e, &[Av1], SoftwareSlow),
            info(Backend::OpenH264, &[H264], SoftwareRealtime),
        ];
        let client = [ClientDecoder::new(
            DecoderKind::WebCodecs,
            &[H264, Av1],
            true,
        )];
        let choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
        assert!(choice
            .candidates
            .iter()
            .all(|c| c.backend != Backend::Rav1e));
        let prefs = SelectionPrefs {
            require_hardware: true,
            ..Default::default()
        };
        assert!(matches!(
            select(&encoders, &client, &prefs),
            Err(CodecError::NoEncoder(_))
        ));
    }
}
