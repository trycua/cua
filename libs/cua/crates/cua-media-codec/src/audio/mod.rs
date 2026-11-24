// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Audio capture, uplink sinks and codecs (plan §8.5, MEDIA.md §12).
//!
//! Canonical sample format across the crate: interleaved signed 16-bit PCM
//! (the `PCM_S16LE` wire format). Backends convert from their native
//! formats (float planar on ScreenCaptureKit, float on WASAPI).
//!
//! Capture is push-based: a backend's [`AudioCapture::start`] spawns its own
//! thread or registers its OS callback and delivers [`AudioFrame`]s to an
//! [`AudioFrameSink`]. Use [`FrameChunker`] to cut arbitrary capture buffers
//! into exact codec frames (10/20/40/60 ms) with contiguous `pts_us`.

pub mod codec;
pub mod packet;

#[cfg(all(target_os = "linux", feature = "pulse"))]
pub mod pulse;

#[cfg(all(target_os = "macos", feature = "sck-audio"))]
pub mod sck;

#[cfg(all(windows, feature = "wasapi"))]
pub mod wasapi;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Sample rate and channel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioFormat {
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Interleaved channels (1 or 2).
    pub channels: u16,
}

impl AudioFormat {
    /// 48 kHz stereo, the downlink default.
    pub const STEREO_48K: Self = Self {
        sample_rate: 48_000,
        channels: 2,
    };
    /// 48 kHz mono, the uplink default.
    pub const MONO_48K: Self = Self {
        sample_rate: 48_000,
        channels: 1,
    };

    /// Samples per channel in `duration_us`.
    pub fn samples_for_us(&self, duration_us: u64) -> usize {
        (u64::from(self.sample_rate) * duration_us / 1_000_000) as usize
    }

    /// Duration of `samples_per_channel` samples in microseconds.
    pub fn duration_us(&self, samples_per_channel: usize) -> u64 {
        samples_per_channel as u64 * 1_000_000 / u64::from(self.sample_rate)
    }
}

/// A block of interleaved s16 samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    /// Media-clock time of the first sample, microseconds.
    pub pts_us: u64,
    /// Format of `samples`.
    pub format: AudioFormat,
    /// Interleaved samples (`samples.len() % channels == 0`).
    pub samples: Vec<i16>,
}

impl AudioFrame {
    /// Samples per channel.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels.max(1))
    }
}

/// Kind of audio source (mirrors `cua.env.v1.AudioSourceKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSourceKind {
    /// The desktop mix.
    Desktop,
    /// One application's audio.
    Application,
}

/// A capturable audio source (mirrors `cua.env.v1.AudioSource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSourceInfo {
    /// Opaque id; `"desktop"` always names the desktop mix.
    pub source_id: String,
    /// What it captures.
    pub kind: AudioSourceKind,
    /// Human-readable name.
    pub name: String,
    /// Owning process id for per-app sources.
    pub pid: Option<u32>,
    /// Owning application bundle id / binary name.
    pub app_id: Option<String>,
    /// Capturable right now.
    pub available: bool,
    /// Why unavailable or degraded.
    pub limitation: Option<String>,
    /// Capturing it actually captures the desktop mix.
    pub desktop_fallback: bool,
}

impl AudioSourceInfo {
    /// The desktop mix entry.
    pub fn desktop(name: impl Into<String>) -> Self {
        Self {
            source_id: DESKTOP_SOURCE_ID.into(),
            kind: AudioSourceKind::Desktop,
            name: name.into(),
            pid: None,
            app_id: None,
            available: true,
            limitation: None,
            desktop_fallback: false,
        }
    }
}

/// Source id of the desktop mix.
pub const DESKTOP_SOURCE_ID: &str = "desktop";

/// Capture backend identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBackend {
    /// PulseAudio or PipeWire's pulse server (Linux).
    Pulse,
    /// ScreenCaptureKit (macOS 13+).
    ScreenCaptureKit,
    /// WASAPI loopback / process loopback (Windows).
    Wasapi,
    /// Tests.
    Fake,
}

/// State changes a capture reports (mirrors `audio_track_state`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCaptureState {
    /// Delivering audio.
    Active,
    /// Capture lost (device removed, sound server restarted); `reason`.
    Suspended(String),
    /// A per-app source is now capturing the desktop mix instead.
    Fallback(String),
    /// The source ended (app exited).
    Ended(String),
}

/// Receives captured audio. Called from the backend's thread.
pub trait AudioFrameSink: Send + Sync {
    /// New samples.
    fn on_audio(&self, frame: AudioFrame);
    /// State change.
    fn on_state(&self, _state: AudioCaptureState) {}
}

impl<F: Fn(AudioFrame) + Send + Sync> AudioFrameSink for F {
    fn on_audio(&self, frame: AudioFrame) {
        self(frame)
    }
}

/// What to capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioCaptureRequest {
    /// `AudioSourceInfo::source_id`.
    pub source_id: String,
    /// Requested output format (backends resample/downmix where they can,
    /// else report their native format in the frames).
    pub format: AudioFormat,
    /// Capture buffer size hint, ms (latency vs wakeups; 10 is typical).
    pub buffer_ms: u32,
}

impl AudioCaptureRequest {
    /// Desktop mix at 48 kHz stereo, 10 ms buffers.
    pub fn desktop() -> Self {
        Self {
            source_id: DESKTOP_SOURCE_ID.into(),
            format: AudioFormat::STEREO_48K,
            buffer_ms: 10,
        }
    }
}

/// A running capture; dropping it stops capture.
pub trait AudioStream: Send {
    /// The source being captured (after any fallback).
    fn source(&self) -> &AudioSourceInfo;
    /// Stops capture and releases OS resources.
    fn stop(&mut self);
}

/// An audio capture backend.
pub trait AudioCapture: Send + Sync {
    /// Backend identity.
    fn backend(&self) -> AudioBackend;
    /// Lists capturable sources, desktop mix first.
    fn sources(&self) -> Result<Vec<AudioSourceInfo>>;
    /// Starts capturing.
    fn start(
        &self,
        request: &AudioCaptureRequest,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Box<dyn AudioStream>>;
}

/// Plays client audio into a guest virtual microphone (uplink).
pub trait AudioSink: Send {
    /// Format expected by [`AudioSink::write`].
    fn format(&self) -> AudioFormat;
    /// Writes interleaved samples (blocks for at most one buffer).
    fn write(&mut self, samples: &[i16]) -> Result<()>;
    /// Name of the virtual source apps see.
    fn source_name(&self) -> &str;
}

/// Opens the platform capture backend, if one is compiled in.
pub fn platform_capture() -> Result<Box<dyn AudioCapture>> {
    #[cfg(all(target_os = "linux", feature = "pulse"))]
    {
        return Ok(Box::new(pulse::PulseCapture::new()?));
    }
    #[cfg(all(target_os = "macos", feature = "sck-audio"))]
    {
        return Ok(Box::new(sck::SckCapture::new()));
    }
    #[cfg(all(windows, feature = "wasapi"))]
    {
        return Ok(Box::new(wasapi::WasapiCapture::new()));
    }
    #[allow(unreachable_code)]
    Err(crate::CodecError::Audio(
        "no audio capture backend in this build".into(),
    ))
}

/// Cuts arbitrary capture buffers into fixed-size frames with contiguous
/// timestamps (MEDIA.md §12.1: `pts_us` advances by exactly the frame
/// duration and only jumps after a gap).
#[derive(Debug)]
pub struct FrameChunker {
    format: AudioFormat,
    frame_samples: usize,
    pending: Vec<i16>,
    next_pts_us: Option<u64>,
    /// Gaps larger than this re-anchor the timeline.
    gap_tolerance_us: u64,
}

impl FrameChunker {
    /// `frame_us` must be a whole number of samples (all Opus durations
    /// are, at 8-48 kHz).
    pub fn new(format: AudioFormat, frame_us: u64) -> Self {
        Self {
            format,
            frame_samples: format.samples_for_us(frame_us),
            pending: Vec::new(),
            next_pts_us: None,
            gap_tolerance_us: frame_us,
        }
    }

    /// Samples per channel per output frame.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Feeds captured samples; returns complete frames.
    pub fn push(&mut self, frame: &AudioFrame) -> Vec<AudioFrame> {
        let ch = usize::from(self.format.channels);
        // Timeline for the first pending sample.
        let buffered_us = self.format.duration_us(self.pending.len() / ch);
        match self.next_pts_us {
            None => self.next_pts_us = Some(frame.pts_us),
            Some(next) => {
                let expected = next + buffered_us;
                if frame.pts_us > expected + self.gap_tolerance_us {
                    // Discontinuity (capture paused / restarted): flush the
                    // partial frame padded with silence and re-anchor.
                    self.pending.clear();
                    self.next_pts_us = Some(frame.pts_us);
                }
            }
        }
        self.pending.extend_from_slice(&frame.samples);
        let per = self.frame_samples * ch;
        let mut out = Vec::new();
        while self.pending.len() >= per {
            let samples: Vec<i16> = self.pending.drain(..per).collect();
            let pts = self.next_pts_us.unwrap_or(0);
            self.next_pts_us = Some(pts + self.format.duration_us(self.frame_samples));
            out.push(AudioFrame {
                pts_us: pts,
                format: self.format,
                samples,
            });
        }
        out
    }
}

/// Converts float samples to s16 with clipping.
pub fn f32_to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunker_emits_exact_frames_with_contiguous_pts() {
        let fmt = AudioFormat::STEREO_48K;
        let mut c = FrameChunker::new(fmt, 20_000);
        assert_eq!(c.frame_samples(), 960);
        let mut out = Vec::new();
        // 7 ms buffers (336 samples/ch).
        for i in 0..20u64 {
            out.extend(c.push(&AudioFrame {
                pts_us: i * 7_000,
                format: fmt,
                samples: vec![i as i16; 336 * 2],
            }));
        }
        assert_eq!(out.len(), 7); // 140 ms -> 7 whole 20 ms frames
        for (i, f) in out.iter().enumerate() {
            assert_eq!(f.pts_us, i as u64 * 20_000);
            assert_eq!(f.frames(), 960);
        }
    }

    #[test]
    fn chunker_reanchors_after_a_gap() {
        let fmt = AudioFormat::MONO_48K;
        let mut c = FrameChunker::new(fmt, 10_000);
        let f = |pts| AudioFrame {
            pts_us: pts,
            format: fmt,
            samples: vec![0; 480],
        };
        assert_eq!(c.push(&f(0))[0].pts_us, 0);
        assert_eq!(c.push(&f(10_000))[0].pts_us, 10_000);
        // 500 ms of silence (DTX / paused capture).
        assert_eq!(c.push(&f(520_000))[0].pts_us, 520_000);
    }
}
