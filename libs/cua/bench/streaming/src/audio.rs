// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Audio side of the benchmark: decode RAU2 tracks to PCM and find tone
//! onsets (the A/V fixture's 1 kHz beeps) on the media clock.

use cua_media_client::decode::{AudioDecoders, EncodedAudio, PcmFrame};
use cua_media_protocol::v2::AudioConfig;
use cua_media_transport::audio::AudioPacketHeader;

/// Detected tone onset.
#[derive(Debug, Clone, Copy)]
pub struct Onset {
    /// Media-clock time of the first loud sample (µs).
    pub pts_us: u64,
    /// Client wall clock when the packet holding it arrived (ns).
    pub arrival_unix_ns: i64,
}

pub struct AudioSide {
    decoders: AudioDecoders,
    pub packets: u64,
    pub bytes: u64,
    pub concealed: u64,
    pub onsets: Vec<Onset>,
    freq_hz: f64,
    /// Samples of silence seen since the last onset (per channel).
    quiet_samples: u64,
    loud: bool,
}

impl AudioSide {
    pub fn new(freq_hz: f64) -> Self {
        Self {
            decoders: AudioDecoders::new(),
            packets: 0,
            bytes: 0,
            concealed: 0,
            onsets: Vec::new(),
            freq_hz,
            quiet_samples: u64::MAX / 2,
            loud: false,
        }
    }

    pub fn configure(&mut self, config: &AudioConfig) {
        let _ = self.decoders.configure(config);
    }

    pub fn packet(&mut self, header: &AudioPacketHeader, payload: &[u8], arrival_unix_ns: i64) {
        self.packets += 1;
        self.bytes += payload.len() as u64 + 24;
        let frames = self.decoders.decode(EncodedAudio {
            track_id: header.track_id,
            sequence: header.sequence,
            pts_us: header.pts_us,
            frame_samples: header.frame_samples,
            config_epoch: header.config_epoch,
            discontinuity: header.discontinuity,
            dtx: header.dtx,
            data: payload,
        });
        for frame in frames {
            if frame.concealed {
                self.concealed += 1;
            }
            self.detect(&frame, arrival_unix_ns);
        }
    }

    fn detect(&mut self, frame: &PcmFrame, arrival_unix_ns: i64) {
        let channels = usize::from(frame.channels.max(1));
        let rate = f64::from(frame.sample_rate.max(1));
        let window = (rate * 0.0025) as usize; // 2.5 ms
        let mono: Vec<f64> = frame
            .samples
            .chunks(channels)
            .map(|c| c.iter().map(|s| f64::from(*s)).sum::<f64>() / channels as f64 / 32768.0)
            .collect();
        let k = 2.0 * (2.0 * std::f64::consts::PI * self.freq_hz / rate).cos();
        for (index, block) in mono.chunks(window.max(1)).enumerate() {
            // Goertzel power at the tone frequency, as an amplitude estimate.
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for x in block {
                let s0 = x + k * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            let power = s1 * s1 + s2 * s2 - k * s1 * s2;
            let amplitude = 2.0 * power.max(0.0).sqrt() / block.len() as f64;
            if amplitude > 0.05 {
                if !self.loud && self.quiet_samples as f64 > rate * 0.2 {
                    let offset = (index * window) as f64 / rate;
                    self.onsets.push(Onset {
                        pts_us: frame.pts_us + (offset * 1e6) as u64,
                        arrival_unix_ns,
                    });
                }
                self.loud = true;
                self.quiet_samples = 0;
            } else {
                self.loud = false;
                self.quiet_samples = self.quiet_samples.saturating_add(block.len() as u64);
            }
        }
    }
}
