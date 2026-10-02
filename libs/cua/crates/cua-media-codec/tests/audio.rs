// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Audio conformance: Opus tone round trips (FFT frequency check), FEC under
//! 10 % loss (SNR), DTX, and the Linux PulseAudio null-sink capture test.
//!
//! Hermetic except `pulse_*`, which needs a PulseAudio server and runs only
//! with `CUA_CODEC_TEST_PULSE=1` (set inside the docker test container).

#![cfg(feature = "opus")]

use cua_media_codec::audio::codec::{
    open_audio_decoder, open_audio_encoder, AudioEncodingConfig, OpusApplication,
};
use cua_media_codec::audio::AudioFormat;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

fn tone(freq: f64, rate: u32, channels: u16, frames: usize, amp: f64) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * usize::from(channels));
    for n in 0..frames {
        let v = (amp
            * 32767.0
            * (2.0 * std::f64::consts::PI * freq * n as f64 / f64::from(rate)).sin())
            as i16;
        for _ in 0..channels {
            out.push(v);
        }
    }
    out
}

fn channel(samples: &[i16], channels: u16, c: usize) -> Vec<f64> {
    samples
        .chunks_exact(usize::from(channels))
        .map(|f| f64::from(f[c]))
        .collect()
}

/// Frequency of the strongest FFT bin (Hann window).
fn peak_hz(signal: &[f64], rate: u32) -> f64 {
    let n = signal.len().next_power_of_two() / 2;
    let start = signal.len() - n;
    let mut buf: Vec<Complex<f64>> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
            Complex::new(signal[start + i] * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let (bin, _) = buf[1..n / 2]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
        .unwrap();
    (bin + 1) as f64 * f64::from(rate) / n as f64
}

/// Best alignment delay (samples) of `decoded` against `reference`.
fn best_delay(reference: &[f64], decoded: &[f64], max_delay: usize) -> usize {
    let window = 4800.min(reference.len() / 2);
    let start = reference.len() / 4;
    (0..=max_delay)
        .max_by(|a, b| {
            let corr = |d: usize| -> f64 {
                (start..start + window)
                    .map(|i| reference[i] * decoded.get(i + d).copied().unwrap_or(0.0))
                    .sum()
            };
            corr(*a).partial_cmp(&corr(*b)).unwrap()
        })
        .unwrap()
}

fn snr_db(reference: &[f64], decoded: &[f64], delay: usize, range: std::ops::Range<usize>) -> f64 {
    let (mut s, mut e) = (0.0, 0.0);
    for i in range {
        let Some(d) = decoded.get(i + delay) else {
            break;
        };
        s += reference[i] * reference[i];
        e += (reference[i] - d) * (reference[i] - d);
    }
    10.0 * (s / e.max(1e-9)).log10()
}

#[test]
fn opus_tone_round_trip_preserves_frequency_for_every_frame_size() {
    for frame_us in [5_000u64, 10_000, 20_000, 40_000, 60_000] {
        let cfg = AudioEncodingConfig {
            frame_us,
            ..AudioEncodingConfig::downlink()
        };
        let mut enc = open_audio_encoder(&cfg).unwrap();
        let mut dec = open_audio_decoder(&cfg).unwrap();
        let fs = cfg.frame_samples();
        let total = 48_000; // 1 s, bounded
        let pcm = tone(1000.0, 48_000, 2, total, 0.5);
        let mut out = Vec::new();
        let mut bytes = 0;
        for chunk in pcm.chunks_exact(fs * 2) {
            let pkt = enc.encode(chunk).unwrap();
            bytes += pkt.data.len();
            out.extend(dec.decode(&pkt.data).unwrap());
        }
        assert_eq!(out.len(), pcm.len() / (fs * 2) * fs * 2);
        let peak = peak_hz(&channel(&out, 2, 0), 48_000);
        let kbps = bytes as f64 * 8.0 / 1000.0;
        eprintln!(
            "frame {} ms: peak {peak:.1} Hz, {kbps:.0} kbit/s, pre-skip {}",
            frame_us as f64 / 1000.0,
            enc.pre_skip()
        );
        assert!(
            (peak - 1000.0).abs() < 15.0,
            "{frame_us}us: peak at {peak} Hz"
        );
        assert!(kbps < 64.0 * 1.3, "bitrate {kbps}");
    }
}

#[test]
fn opus_tone_sequence_frequencies_survive() {
    // A short sequence of tones (the Spaces fixture pattern): 440, 880, 1320 Hz.
    let cfg = AudioEncodingConfig::downlink();
    let mut enc = open_audio_encoder(&cfg).unwrap();
    let mut dec = open_audio_decoder(&cfg).unwrap();
    for f in [440.0, 880.0, 1320.0] {
        let pcm = tone(f, 48_000, 2, 19_200, 0.4); // 400 ms
        let mut out = Vec::new();
        for chunk in pcm.as_chunks::<1920>().0 {
            out.extend(dec.decode(&enc.encode(chunk).unwrap().data).unwrap());
        }
        let peak = peak_hz(&channel(&out, 2, 1), 48_000);
        assert!((peak - f).abs() < 15.0, "{f} Hz tone decoded at {peak} Hz");
    }
}

/// FEC under 10 % simulated loss: recovering lost frames from the next
/// packet's in-band FEC must beat plain concealment and stay above an SNR
/// floor. Uses a speech-range configuration (mono, 24 kbit/s), where
/// libopus codes SILK/hybrid and emits LBRR.
#[test]
fn opus_fec_recovers_under_ten_percent_loss() {
    let cfg = AudioEncodingConfig {
        format: AudioFormat::MONO_48K,
        bitrate_kbps: 24,
        fec: true,
        dtx: false,
        expected_loss_percent: 10,
        application: OpusApplication::Voip,
        ..AudioEncodingConfig::downlink()
    };
    let fs = cfg.frame_samples();
    let frames = 150; // 3 s, bounded
    let pcm = tone(440.0, 48_000, 1, fs * frames, 0.5);
    let mut enc = open_audio_encoder(&cfg).unwrap();
    let packets: Vec<Vec<u8>> = pcm
        .chunks_exact(fs)
        .map(|c| enc.encode(c).unwrap().data)
        .collect();
    // Exactly 10 % loss: one packet in every block of 10, at an offset
    // that varies per block (xorshift), never two in a row, never the
    // first or last packet.
    let mut state = 0x2545_f491u32;
    let mut lost = vec![false; frames];
    for block in 0..frames / 10 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let i = (block * 10 + 1 + (state >> 8) as usize % 8).min(frames - 2);
        if !lost[i - 1] {
            lost[i] = true;
        }
    }
    let loss_count = lost.iter().filter(|l| **l).count();
    let run = |use_fec: bool| -> Vec<f64> {
        let mut dec = open_audio_decoder(&cfg).unwrap();
        let mut out: Vec<i16> = Vec::new();
        for i in 0..frames {
            if lost[i] {
                let rebuilt = if use_fec {
                    dec.decode_fec(&packets[i + 1], fs)
                } else {
                    dec.conceal(fs)
                };
                out.extend(rebuilt.unwrap());
            } else {
                out.extend(dec.decode(&packets[i]).unwrap());
            }
        }
        out.iter().map(|s| f64::from(*s)).collect()
    };
    let reference: Vec<f64> = pcm.iter().map(|s| f64::from(*s)).collect();
    let clean = {
        let mut dec = open_audio_decoder(&cfg).unwrap();
        let v: Vec<i16> = packets
            .iter()
            .flat_map(|p| dec.decode(p).unwrap())
            .collect();
        v.iter().map(|s| f64::from(*s)).collect::<Vec<f64>>()
    };
    let delay = best_delay(&reference, &clean, 960);
    // SNR measured over the lost frames only (where recovery matters).
    let lost_snr = |decoded: &[f64]| -> f64 {
        let mut vals = Vec::new();
        for (i, l) in lost.iter().enumerate() {
            if *l && (i + 1) * fs + delay < decoded.len() {
                vals.push(snr_db(&reference, decoded, delay, i * fs..(i + 1) * fs));
            }
        }
        vals.iter().sum::<f64>() / vals.len() as f64
    };
    let with_fec = run(true);
    let with_plc = run(false);
    let clean_snr = snr_db(&reference, &clean, delay, fs * 5..fs * (frames - 5));
    let fec_overall = snr_db(&reference, &with_fec, delay, fs * 5..fs * (frames - 5));
    let fec_lost = lost_snr(&with_fec);
    let plc_lost = lost_snr(&with_plc);
    let peak = peak_hz(&with_fec, 48_000);
    eprintln!(
        "loss {loss_count}/{frames}, delay {delay}: clean {clean_snr:.1} dB, FEC overall {fec_overall:.1} dB, lost frames FEC {fec_lost:.1} dB vs PLC {plc_lost:.1} dB, peak {peak:.1} Hz"
    );
    assert!(
        (13..=15).contains(&loss_count),
        "loss pattern {loss_count} is not ~10 %"
    );
    assert!(
        fec_lost > plc_lost + 3.0,
        "FEC {fec_lost:.1} dB should beat PLC {plc_lost:.1} dB"
    );
    assert!(
        fec_lost > 8.0,
        "FEC SNR on lost frames {fec_lost:.1} dB below 8 dB"
    );
    assert!(fec_overall > 10.0, "overall SNR {fec_overall:.1} dB");
    assert!((peak - 440.0).abs() < 15.0);
}

#[test]
fn opus_dtx_suppresses_silence() {
    let cfg = AudioEncodingConfig::downlink();
    let mut enc = open_audio_encoder(&cfg).unwrap();
    let silence = vec![0i16; 960 * 2];
    let flagged = (0..50)
        .filter(|_| enc.encode(&silence).unwrap().dtx_silence)
        .count();
    assert!(
        flagged > 30,
        "only {flagged}/50 silent frames flagged for DTX"
    );
    let mut enc = open_audio_encoder(&AudioEncodingConfig { dtx: false, ..cfg }).unwrap();
    assert_eq!(
        (0..50)
            .filter(|_| enc.encode(&silence).unwrap().dtx_silence)
            .count(),
        0
    );
}

#[test]
fn opus_live_reconfiguration() {
    let mut enc = open_audio_encoder(&AudioEncodingConfig::downlink()).unwrap();
    enc.set_bitrate_kbps(128).unwrap();
    enc.set_fec(false, 0).unwrap();
    enc.set_dtx(false).unwrap();
    assert_eq!(enc.config().bitrate_kbps, 128);
    assert!(enc.set_bitrate_kbps(1000).is_err());
}

/// Linux PulseAudio null-sink capture: start capture of the desktop monitor,
/// play a tone with `paplay`, verify the tone frequency.
#[cfg(all(target_os = "linux", feature = "pulse"))]
mod pulse {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use cua_media_codec::audio::pulse::{ensure_desktop_sink, PulseCapture, PulseUplink};
    use cua_media_codec::audio::{
        AudioCapture, AudioCaptureRequest, AudioFormat, AudioFrame, AudioFrameSink, AudioSink,
    };

    use super::*;

    fn enabled() -> bool {
        std::env::var_os("CUA_CODEC_TEST_PULSE").is_some()
    }

    /// The tests share one sound server; run them one at a time.
    static SERVER: Mutex<()> = Mutex::new(());

    fn write_wav(path: &std::path::Path, samples: &[i16], rate: u32, channels: u16) {
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        b.extend_from_slice(&(channels * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// Collects at most `max` samples (bounded memory).
    struct Collect {
        buf: Mutex<Vec<i16>>,
        max: usize,
    }
    impl AudioFrameSink for Collect {
        fn on_audio(&self, f: AudioFrame) {
            let mut b = self.buf.lock().unwrap();
            if b.len() < self.max {
                b.extend(f.samples);
            }
        }
    }

    fn capture_until(stream_samples: &Arc<Collect>, want: usize, timeout: Duration) {
        let start = Instant::now();
        while stream_samples.buf.lock().unwrap().len() < want && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn pulse_null_sink_monitor_captures_played_tone() {
        if !enabled() {
            eprintln!("skipped: needs a PulseAudio server (CUA_CODEC_TEST_PULSE=1)");
            return;
        }
        let _serial = SERVER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sink_name = ensure_desktop_sink().unwrap();
        eprintln!("default sink: {sink_name}");
        let capture = PulseCapture::new().unwrap();
        let sources = capture.sources().unwrap();
        assert_eq!(sources[0].source_id, "desktop");
        let collected = Arc::new(Collect {
            buf: Mutex::new(Vec::new()),
            max: 48_000 * 2 * 4,
        });
        let mut stream = capture
            .start(&AudioCaptureRequest::desktop(), collected.clone())
            .unwrap();
        let wav = std::env::temp_dir().join("cua-codec-tone.wav");
        write_wav(&wav, &tone(1000.0, 48_000, 2, 48_000 * 2, 0.5), 48_000, 2);
        let status = std::process::Command::new("paplay")
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success());
        capture_until(&collected, 48_000 * 2 * 2, Duration::from_secs(10));
        stream.stop();
        let samples = collected.buf.lock().unwrap().clone();
        // Keep the loudest second (the tone).
        let left = channel(&samples, 2, 0);
        let energy = |s: &[f64]| s.iter().map(|x| x * x).sum::<f64>();
        assert!(left.len() >= 48_000, "captured only {} frames", left.len());
        let best = (0..=left.len() - 48_000)
            .step_by(4_800)
            .max_by(|a, b| {
                energy(&left[*a..*a + 48_000])
                    .partial_cmp(&energy(&left[*b..*b + 48_000]))
                    .unwrap()
            })
            .unwrap();
        let window = &left[best..best + 48_000];
        let rms = (energy(window) / window.len() as f64).sqrt();
        let peak = peak_hz(window, 48_000);
        eprintln!(
            "captured {} frames, rms {rms:.0}, peak {peak:.1} Hz",
            left.len()
        );
        assert!(rms > 1000.0, "tone not captured (rms {rms})");
        assert!((peak - 1000.0).abs() < 15.0, "peak {peak}");

        // Opus the captured audio and check it still decodes to 1 kHz.
        let cfg = AudioEncodingConfig::downlink();
        let mut enc = open_audio_encoder(&cfg).unwrap();
        let mut dec = open_audio_decoder(&cfg).unwrap();
        let start = best * 2;
        let mut out = Vec::new();
        for chunk in samples[start..start + 96_000].as_chunks::<1920>().0.iter() {
            out.extend(dec.decode(&enc.encode(chunk).unwrap().data).unwrap());
        }
        assert!((peak_hz(&channel(&out, 2, 0), 48_000) - 1000.0).abs() < 15.0);
    }

    #[test]
    fn pulse_uplink_virtual_source_round_trip() {
        if !enabled() {
            eprintln!("skipped: needs a PulseAudio server (CUA_CODEC_TEST_PULSE=1)");
            return;
        }
        let _serial = SERVER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_desktop_sink().unwrap();
        let fmt = AudioFormat::MONO_48K;
        let mut uplink = PulseUplink::open("cua-test-mic", fmt, false).unwrap();
        // Record from the virtual microphone like an app would.
        let capture = PulseCapture::new().unwrap();
        let collected = Arc::new(Collect {
            buf: Mutex::new(Vec::new()),
            max: 48_000 * 4,
        });
        let req = AudioCaptureRequest {
            source_id: "desktop".into(),
            format: fmt,
            buffer_ms: 10,
        };
        // Capture the virtual source directly by device name.
        let mut stream = cua_media_codec::audio::pulse::record_device(
            &capture,
            "cua-test-mic",
            &req,
            collected.clone(),
        )
        .unwrap();
        let t = tone(660.0, 48_000, 1, 48_000, 0.5);
        for chunk in t.chunks(960) {
            uplink.write(chunk).unwrap();
        }
        capture_until(&collected, 48_000, Duration::from_secs(10));
        stream.stop();
        drop(uplink);
        let got: Vec<f64> = collected
            .buf
            .lock()
            .unwrap()
            .iter()
            .map(|s| f64::from(*s))
            .collect();
        assert!(got.len() >= 24_000, "captured {}", got.len());
        let peak = peak_hz(&got, 48_000);
        eprintln!("uplink captured {} samples, peak {peak:.1} Hz", got.len());
        assert!((peak - 660.0).abs() < 15.0);
    }
}
