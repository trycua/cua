// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Audio encoders/decoders: Opus (libopus, BSD) and PCM s16le passthrough.
//!
//! Defaults follow `cua.env.v1.AudioEncoding`: 48 kHz, stereo downlink /
//! mono uplink, 64/32 kbit/s, 20 ms frames, in-band FEC on (10 % expected
//! loss), DTX on.
//!
//! **FEC and low-delay mode.** Opus in-band FEC (LBRR) exists only in the
//! SILK and hybrid layers. `RESTRICTED_LOWDELAY` is CELT-only, so FEC would
//! silently do nothing there. [`OpusApplication::Auto`] therefore selects
//! restricted-low-delay only when FEC is off; with FEC on it selects the
//! `AUDIO` application, which lets libopus pick SILK/hybrid (and emit FEC)
//! at speech-range bitrates. At high stereo bitrates libopus stays in CELT
//! and FEC has no effect; lossy links should lower the bitrate (or rely on
//! transport FEC/RED).

use serde::{Deserialize, Serialize};

use super::AudioFormat;
use crate::error::{CodecError, Result};

/// Audio codec (mirrors `cua.env.v1.AudioCodec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodecKind {
    /// Opus (RFC 6716).
    Opus,
    /// Interleaved s16le PCM.
    PcmS16le,
}

/// Opus application mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpusApplication {
    /// Restricted low delay unless FEC is on (see module docs).
    #[default]
    Auto,
    /// CELT-only, lowest algorithmic delay; no in-band FEC.
    RestrictedLowDelay,
    /// General audio.
    Audio,
    /// Speech-optimised.
    Voip,
}

/// Opus frame durations in microseconds.
pub const OPUS_FRAME_US: [u64; 6] = [2_500, 5_000, 10_000, 20_000, 40_000, 60_000];

/// Encoding parameters with defaults applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioEncodingConfig {
    /// Codec.
    pub codec: AudioCodecKind,
    /// Sample rate and channels.
    pub format: AudioFormat,
    /// Opus target bitrate, kbit/s (6..=510). Ignored for PCM.
    pub bitrate_kbps: u32,
    /// Opus in-band FEC.
    pub fec: bool,
    /// Opus DTX.
    pub dtx: bool,
    /// Frame duration in microseconds (one of [`OPUS_FRAME_US`]).
    pub frame_us: u64,
    /// Expected loss 0..=100 (tunes FEC redundancy).
    pub expected_loss_percent: u8,
    /// Opus application mode.
    pub application: OpusApplication,
    /// Opus complexity 0..=10.
    pub complexity: u8,
}

impl Default for AudioEncodingConfig {
    fn default() -> Self {
        Self::downlink()
    }
}

impl AudioEncodingConfig {
    /// Downlink default: Opus 48 kHz stereo, 64 kbit/s, 20 ms, FEC + DTX.
    pub fn downlink() -> Self {
        Self {
            codec: AudioCodecKind::Opus,
            format: AudioFormat::STEREO_48K,
            bitrate_kbps: 64,
            fec: true,
            dtx: true,
            frame_us: 20_000,
            expected_loss_percent: 10,
            application: OpusApplication::Auto,
            complexity: 5,
        }
    }

    /// Uplink default: Opus 48 kHz mono, 32 kbit/s, 20 ms, FEC + DTX.
    pub fn uplink() -> Self {
        Self {
            format: AudioFormat::MONO_48K,
            bitrate_kbps: 32,
            ..Self::downlink()
        }
    }

    /// Frame duration in whole milliseconds (rounded down; 2.5 ms -> 2).
    pub fn frame_ms(&self) -> u32 {
        (self.frame_us / 1000) as u32
    }

    /// Samples per channel per frame.
    pub fn frame_samples(&self) -> usize {
        self.format.samples_for_us(self.frame_us)
    }

    /// The application mode libopus actually uses.
    pub fn effective_application(&self) -> OpusApplication {
        match self.application {
            OpusApplication::Auto if self.fec => OpusApplication::Audio,
            OpusApplication::Auto => OpusApplication::RestrictedLowDelay,
            other => other,
        }
    }

    /// Validates rates and durations.
    pub fn validate(&self) -> Result<()> {
        if !(1..=2).contains(&self.format.channels) {
            return Err(CodecError::InvalidArgument(
                "channels must be 1 or 2".into(),
            ));
        }
        match self.codec {
            AudioCodecKind::Opus => {
                if ![8_000, 12_000, 16_000, 24_000, 48_000].contains(&self.format.sample_rate) {
                    return Err(CodecError::InvalidArgument(format!(
                        "Opus sample rate {} not in 8/12/16/24/48 kHz",
                        self.format.sample_rate
                    )));
                }
                if !OPUS_FRAME_US.contains(&self.frame_us) {
                    return Err(CodecError::InvalidArgument(format!(
                        "Opus frame duration {} us not in 2.5/5/10/20/40/60 ms",
                        self.frame_us
                    )));
                }
                if !(6..=510).contains(&self.bitrate_kbps) {
                    return Err(CodecError::InvalidArgument(
                        "Opus bitrate must be 6..=510 kbit/s".into(),
                    ));
                }
            }
            AudioCodecKind::PcmS16le => {
                if !(8_000..=48_000).contains(&self.format.sample_rate) || self.frame_samples() == 0
                {
                    return Err(CodecError::InvalidArgument(
                        "PCM rate must be 8..=48 kHz".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One encoded audio packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedAudio {
    /// Codec payload (one Opus packet, or raw s16le).
    pub data: Vec<u8>,
    /// Opus DTX: this frame is silence and need not be transmitted (the
    /// encoder produced a 1-2 byte packet). Senders skip it; `pts_us` of
    /// the next sent packet jumps forward (MEDIA.md §12.4).
    pub dtx_silence: bool,
}

/// An audio encoder.
pub trait AudioEncoder: Send {
    /// Active configuration.
    fn config(&self) -> &AudioEncodingConfig;
    /// Encodes exactly `frame_samples() * channels` interleaved samples.
    fn encode(&mut self, pcm: &[i16]) -> Result<EncodedAudio>;
    /// Live bitrate change.
    fn set_bitrate_kbps(&mut self, kbps: u32) -> Result<()>;
    /// Live FEC toggle / expected loss.
    fn set_fec(&mut self, fec: bool, expected_loss_percent: u8) -> Result<()>;
    /// Live DTX toggle.
    fn set_dtx(&mut self, dtx: bool) -> Result<()>;
    /// Decoder pre-skip in samples at 48 kHz (`audio_config.opus_pre_skip`).
    fn pre_skip(&self) -> u32;
}

/// An audio decoder.
pub trait AudioDecoder: Send {
    /// Output format.
    fn format(&self) -> AudioFormat;
    /// Decodes one packet into interleaved samples.
    fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>>;
    /// Rebuilds a lost frame of `frame_samples` from the FEC data carried in
    /// the packet that followed it. Falls back to concealment when the
    /// packet carries no FEC.
    fn decode_fec(&mut self, next_packet: &[u8], frame_samples: usize) -> Result<Vec<i16>>;
    /// Packet-loss concealment for a missing frame (silence for PCM).
    fn conceal(&mut self, frame_samples: usize) -> Result<Vec<i16>>;
}

/// Opens an encoder for `config`.
pub fn open_audio_encoder(config: &AudioEncodingConfig) -> Result<Box<dyn AudioEncoder>> {
    config.validate()?;
    match config.codec {
        #[cfg(feature = "opus")]
        AudioCodecKind::Opus => Ok(Box::new(opus::OpusEncoder::new(config)?)),
        #[cfg(not(feature = "opus"))]
        AudioCodecKind::Opus => Err(CodecError::Unsupported(
            "built without the `opus` feature".into(),
        )),
        AudioCodecKind::PcmS16le => Ok(Box::new(PcmCodec::new(config.clone()))),
    }
}

/// Opens a decoder for `config`.
pub fn open_audio_decoder(config: &AudioEncodingConfig) -> Result<Box<dyn AudioDecoder>> {
    config.validate()?;
    match config.codec {
        #[cfg(feature = "opus")]
        AudioCodecKind::Opus => Ok(Box::new(opus::OpusDecoder::new(config.format)?)),
        #[cfg(not(feature = "opus"))]
        AudioCodecKind::Opus => Err(CodecError::Unsupported(
            "built without the `opus` feature".into(),
        )),
        AudioCodecKind::PcmS16le => Ok(Box::new(PcmCodec::new(config.clone()))),
    }
}

/// PCM s16le passthrough (debug / lossless).
#[derive(Debug, Clone)]
pub struct PcmCodec {
    config: AudioEncodingConfig,
}

impl PcmCodec {
    /// New passthrough codec.
    pub fn new(mut config: AudioEncodingConfig) -> Self {
        config.codec = AudioCodecKind::PcmS16le;
        config.fec = false;
        config.dtx = false;
        config.bitrate_kbps = 0;
        Self { config }
    }

    /// The normalised configuration.
    pub fn config_clone(&self) -> AudioEncodingConfig {
        self.config.clone()
    }
}

impl AudioEncoder for PcmCodec {
    fn config(&self) -> &AudioEncodingConfig {
        &self.config
    }
    fn encode(&mut self, pcm: &[i16]) -> Result<EncodedAudio> {
        let mut data = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            data.extend_from_slice(&s.to_le_bytes());
        }
        Ok(EncodedAudio {
            data,
            dtx_silence: false,
        })
    }
    fn set_bitrate_kbps(&mut self, _kbps: u32) -> Result<()> {
        Ok(())
    }
    fn set_fec(&mut self, _fec: bool, _loss: u8) -> Result<()> {
        Ok(())
    }
    fn set_dtx(&mut self, _dtx: bool) -> Result<()> {
        Ok(())
    }
    fn pre_skip(&self) -> u32 {
        0
    }
}

impl AudioDecoder for PcmCodec {
    fn format(&self) -> AudioFormat {
        self.config.format
    }
    fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        if !packet
            .len()
            .is_multiple_of(2 * usize::from(self.config.format.channels))
        {
            return Err(CodecError::InvalidArgument(
                "PCM payload length is not a whole frame".into(),
            ));
        }
        Ok(packet
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect())
    }
    fn decode_fec(&mut self, _next: &[u8], frame_samples: usize) -> Result<Vec<i16>> {
        self.conceal(frame_samples)
    }
    fn conceal(&mut self, frame_samples: usize) -> Result<Vec<i16>> {
        Ok(vec![
            0;
            frame_samples * usize::from(self.config.format.channels)
        ])
    }
}

#[cfg(feature = "opus")]
pub mod opus {
    //! Opus through audiopus_sys (ISC) / libopus (BSD).

    use std::ffi::c_int;
    use std::ptr::NonNull;

    use audiopus_sys as sys;

    use super::{AudioDecoder, AudioEncoder, AudioEncodingConfig, EncodedAudio, OpusApplication};
    use crate::audio::AudioFormat;
    use crate::error::{CodecError, Result};

    const MAX_PACKET: usize = 1500;

    fn err(what: &str, code: c_int) -> CodecError {
        CodecError::Audio(format!("{what}: opus error {code}"))
    }

    /// Opus encoder.
    pub struct OpusEncoder {
        st: NonNull<sys::OpusEncoder>,
        config: AudioEncodingConfig,
        pre_skip: u32,
        out: Vec<u8>,
    }

    // SAFETY: an OpusEncoder has no thread affinity; access is &mut only.
    unsafe impl Send for OpusEncoder {}

    impl OpusEncoder {
        /// Creates an encoder.
        pub fn new(config: &AudioEncodingConfig) -> Result<Self> {
            config.validate()?;
            let app = match config.effective_application() {
                OpusApplication::RestrictedLowDelay | OpusApplication::Auto => {
                    sys::OPUS_APPLICATION_RESTRICTED_LOWDELAY
                }
                OpusApplication::Audio => sys::OPUS_APPLICATION_AUDIO,
                OpusApplication::Voip => sys::OPUS_APPLICATION_VOIP,
            };
            let mut error = 0;
            let st = unsafe {
                sys::opus_encoder_create(
                    config.format.sample_rate as i32,
                    c_int::from(config.format.channels),
                    app,
                    &mut error,
                )
            };
            let st = NonNull::new(st)
                .filter(|_| error == 0)
                .ok_or_else(|| err("opus_encoder_create", error))?;
            let mut me = Self {
                st,
                config: config.clone(),
                pre_skip: 0,
                out: vec![0; MAX_PACKET],
            };
            me.ctl(
                sys::OPUS_SET_COMPLEXITY_REQUEST,
                c_int::from(config.complexity.min(10)),
            )?;
            me.set_bitrate_kbps(config.bitrate_kbps)?;
            me.set_fec(config.fec, config.expected_loss_percent)?;
            me.set_dtx(config.dtx)?;
            let mut lookahead: c_int = 0;
            let rc = unsafe {
                sys::opus_encoder_ctl(
                    me.st.as_ptr(),
                    sys::OPUS_GET_LOOKAHEAD_REQUEST,
                    &mut lookahead as *mut c_int,
                )
            };
            if rc != 0 {
                return Err(err("OPUS_GET_LOOKAHEAD", rc));
            }
            // Pre-skip is expressed at 48 kHz (RFC 7845).
            me.pre_skip = (lookahead as u32) * (48_000 / config.format.sample_rate);
            Ok(me)
        }

        fn ctl(&mut self, request: c_int, value: c_int) -> Result<()> {
            let rc = unsafe { sys::opus_encoder_ctl(self.st.as_ptr(), request, value) };
            if rc == 0 {
                Ok(())
            } else {
                Err(err("opus_encoder_ctl", rc))
            }
        }
    }

    impl Drop for OpusEncoder {
        fn drop(&mut self) {
            unsafe { sys::opus_encoder_destroy(self.st.as_ptr()) };
        }
    }

    impl AudioEncoder for OpusEncoder {
        fn config(&self) -> &AudioEncodingConfig {
            &self.config
        }

        fn encode(&mut self, pcm: &[i16]) -> Result<EncodedAudio> {
            let frame = self.config.frame_samples();
            if pcm.len() != frame * usize::from(self.config.format.channels) {
                return Err(CodecError::InvalidArgument(format!(
                    "Opus frame needs {} samples per channel, got {}",
                    frame,
                    pcm.len() / usize::from(self.config.format.channels)
                )));
            }
            let n = unsafe {
                sys::opus_encode(
                    self.st.as_ptr(),
                    pcm.as_ptr(),
                    frame as c_int,
                    self.out.as_mut_ptr(),
                    self.out.len() as i32,
                )
            };
            if n < 0 {
                return Err(err("opus_encode", n));
            }
            let data = self.out[..n as usize].to_vec();
            Ok(EncodedAudio {
                dtx_silence: self.config.dtx && n <= 2,
                data,
            })
        }

        fn set_bitrate_kbps(&mut self, kbps: u32) -> Result<()> {
            if !(6..=510).contains(&kbps) {
                return Err(CodecError::InvalidArgument(
                    "Opus bitrate must be 6..=510 kbit/s".into(),
                ));
            }
            self.ctl(sys::OPUS_SET_BITRATE_REQUEST, (kbps * 1000) as c_int)?;
            self.config.bitrate_kbps = kbps;
            Ok(())
        }

        fn set_fec(&mut self, fec: bool, expected_loss_percent: u8) -> Result<()> {
            self.ctl(sys::OPUS_SET_INBAND_FEC_REQUEST, c_int::from(fec))?;
            let loss = if fec {
                expected_loss_percent.min(100)
            } else {
                0
            };
            self.ctl(sys::OPUS_SET_PACKET_LOSS_PERC_REQUEST, c_int::from(loss))?;
            self.config.fec = fec;
            self.config.expected_loss_percent = expected_loss_percent;
            Ok(())
        }

        fn set_dtx(&mut self, dtx: bool) -> Result<()> {
            self.ctl(sys::OPUS_SET_DTX_REQUEST, c_int::from(dtx))?;
            self.config.dtx = dtx;
            Ok(())
        }

        fn pre_skip(&self) -> u32 {
            self.pre_skip
        }
    }

    /// Opus decoder.
    pub struct OpusDecoder {
        st: NonNull<sys::OpusDecoder>,
        format: AudioFormat,
        buf: Vec<i16>,
    }

    // SAFETY: as for the encoder.
    unsafe impl Send for OpusDecoder {}

    impl OpusDecoder {
        /// Creates a decoder producing `format`.
        pub fn new(format: AudioFormat) -> Result<Self> {
            let mut error = 0;
            let st = unsafe {
                sys::opus_decoder_create(
                    format.sample_rate as i32,
                    c_int::from(format.channels),
                    &mut error,
                )
            };
            let st = NonNull::new(st)
                .filter(|_| error == 0)
                .ok_or_else(|| err("opus_decoder_create", error))?;
            Ok(Self {
                st,
                format,
                // 120 ms is the largest Opus packet duration.
                buf: vec![0; format.samples_for_us(120_000) * usize::from(format.channels)],
            })
        }

        fn run(
            &mut self,
            data: Option<&[u8]>,
            frame_samples: usize,
            fec: bool,
        ) -> Result<Vec<i16>> {
            let max = if fec || data.is_none() {
                frame_samples
            } else {
                self.buf.len() / usize::from(self.format.channels)
            };
            let (ptr, len) = data.map_or((std::ptr::null(), 0), |d| (d.as_ptr(), d.len() as i32));
            let n = unsafe {
                sys::opus_decode(
                    self.st.as_ptr(),
                    ptr,
                    len,
                    self.buf.as_mut_ptr(),
                    max as c_int,
                    c_int::from(fec),
                )
            };
            if n < 0 {
                return Err(err("opus_decode", n));
            }
            Ok(self.buf[..n as usize * usize::from(self.format.channels)].to_vec())
        }
    }

    impl Drop for OpusDecoder {
        fn drop(&mut self) {
            unsafe { sys::opus_decoder_destroy(self.st.as_ptr()) };
        }
    }

    impl AudioDecoder for OpusDecoder {
        fn format(&self) -> AudioFormat {
            self.format
        }
        fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
            self.run(Some(packet), 0, false)
        }
        fn decode_fec(&mut self, next_packet: &[u8], frame_samples: usize) -> Result<Vec<i16>> {
            self.run(Some(next_packet), frame_samples, true)
        }
        fn conceal(&mut self, frame_samples: usize) -> Result<Vec<i16>> {
            self.run(None, frame_samples, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_contract() {
        let d = AudioEncodingConfig::downlink();
        assert_eq!(
            (
                d.format.sample_rate,
                d.format.channels,
                d.bitrate_kbps,
                d.frame_ms()
            ),
            (48_000, 2, 64, 20)
        );
        assert!(d.fec && d.dtx);
        assert_eq!(d.frame_samples(), 960);
        let u = AudioEncodingConfig::uplink();
        assert_eq!((u.format.channels, u.bitrate_kbps), (1, 32));
    }

    #[test]
    fn auto_application_avoids_celt_only_when_fec_is_on() {
        let mut c = AudioEncodingConfig::downlink();
        assert_eq!(c.effective_application(), OpusApplication::Audio);
        c.fec = false;
        assert_eq!(
            c.effective_application(),
            OpusApplication::RestrictedLowDelay
        );
    }

    #[test]
    fn validation_rejects_bad_frames() {
        let mut c = AudioEncodingConfig::downlink();
        c.frame_us = 15_000;
        assert!(c.validate().is_err());
        for us in [5_000, 10_000, 20_000, 40_000, 60_000] {
            c.frame_us = us;
            c.validate().unwrap();
        }
    }

    #[test]
    fn pcm_round_trip_is_bit_exact() {
        let cfg = AudioEncodingConfig {
            codec: AudioCodecKind::PcmS16le,
            ..AudioEncodingConfig::downlink()
        };
        let mut enc = open_audio_encoder(&cfg).unwrap();
        let mut dec = open_audio_decoder(&cfg).unwrap();
        let pcm: Vec<i16> = (0..1920).map(|i| (i * 37 % 65536 - 32768) as i16).collect();
        let packet = enc.encode(&pcm).unwrap();
        assert_eq!(packet.data.len(), 1920 * 2);
        assert_eq!(dec.decode(&packet.data).unwrap(), pcm);
        assert!(dec.decode(&[1, 2, 3]).is_err());
    }
}
