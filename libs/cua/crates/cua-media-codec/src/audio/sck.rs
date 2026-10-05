// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! ScreenCaptureKit audio capture (macOS 13+): desktop mix and per-app.
//!
//! Requires the Screen Recording TCC permission and captures the real
//! system audio. Tests that start a capture are gated behind
//! `CUA_ENV_ALLOW_HOST_APP_EFFECTS=1` and must not run on a developer's
//! daily machine.
//!
//! SCK always runs a video stream; we request a 2x2 surface at 1 fps so
//! the cost is negligible, and only attach an audio output handler. The
//! current process's own audio is excluded (feedback prevention). Samples
//! arrive as 32-bit float, usually one buffer per channel; they are
//! interleaved and converted to s16.

use std::sync::Arc;

use screencapturekit::cm::CMSampleBufferExt;
use screencapturekit::prelude::*;

use super::{
    f32_to_i16, AudioBackend, AudioCapture, AudioCaptureRequest, AudioCaptureState, AudioFormat,
    AudioFrame, AudioFrameSink, AudioSourceInfo, AudioSourceKind, AudioStream, DESKTOP_SOURCE_ID,
};
use crate::error::{CodecError, Result};

/// ScreenCaptureKit audio backend.
#[derive(Debug, Default)]
pub struct SckCapture;

impl SckCapture {
    /// New backend (no system calls until used).
    pub fn new() -> Self {
        Self
    }
}

fn content() -> Result<SCShareableContent> {
    SCShareableContent::get().map_err(|e| CodecError::Audio(format!("SCShareableContent: {e:?}")))
}

struct SckStream {
    source: AudioSourceInfo,
    stream: Option<SCStream>,
}

// SAFETY: SCStream is an Objective-C object usable from any thread.
unsafe impl Send for SckStream {}

impl AudioStream for SckStream {
    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }
    fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.stop_capture();
        }
    }
}

impl Drop for SckStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Interleaves an SCK audio buffer list into s16 samples.
pub fn interleave_f32(planes: &[&[u8]], channels_per_plane: u32, out_channels: u16) -> Vec<i16> {
    let floats = |p: &[u8]| -> Vec<f32> {
        p.as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    };
    let out_ch = usize::from(out_channels.max(1));
    if planes.len() == 1 {
        // Already interleaved with `channels_per_plane` channels.
        let src = floats(planes[0]);
        let in_ch = channels_per_plane.max(1) as usize;
        let frames = src.len() / in_ch;
        let mut out = Vec::with_capacity(frames * out_ch);
        for f in 0..frames {
            for c in 0..out_ch {
                out.push(f32_to_i16(src[f * in_ch + c.min(in_ch - 1)]));
            }
        }
        return out;
    }
    let chans: Vec<Vec<f32>> = planes.iter().map(|p| floats(p)).collect();
    let frames = chans.iter().map(Vec::len).min().unwrap_or(0);
    let mut out = Vec::with_capacity(frames * out_ch);
    let last = chans.len() - 1;
    #[allow(clippy::needless_range_loop)] // indexes several planes per frame
    for f in 0..frames {
        out.extend((0..out_ch).map(|c| f32_to_i16(chans[c.min(last)][f])));
    }
    out
}

impl AudioCapture for SckCapture {
    fn backend(&self) -> AudioBackend {
        AudioBackend::ScreenCaptureKit
    }

    fn sources(&self) -> Result<Vec<AudioSourceInfo>> {
        let content = content()?;
        let mut out = vec![AudioSourceInfo::desktop("Desktop audio")];
        for app in content.applications() {
            let pid = app.process_id();
            if pid <= 0 {
                continue;
            }
            out.push(AudioSourceInfo {
                source_id: format!("app:{pid}"),
                kind: AudioSourceKind::Application,
                name: app.application_name(),
                pid: Some(pid as u32),
                app_id: Some(app.bundle_identifier()),
                available: true,
                limitation: None,
                desktop_fallback: false,
            });
        }
        Ok(out)
    }

    fn start(
        &self,
        request: &AudioCaptureRequest,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Box<dyn AudioStream>> {
        let content = content()?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| CodecError::Audio("no display for ScreenCaptureKit".into()))?;
        let (filter, source) = if request.source_id == DESKTOP_SOURCE_ID {
            (
                SCContentFilter::create()
                    .with_display(&display)
                    .with_excluding_windows(&[])
                    .build(),
                AudioSourceInfo::desktop("Desktop audio"),
            )
        } else {
            let pid: i32 = request
                .source_id
                .strip_prefix("app:")
                .and_then(|p| p.parse().ok())
                .ok_or_else(|| {
                    CodecError::InvalidArgument(format!(
                        "unknown audio source {}",
                        request.source_id
                    ))
                })?;
            let apps = content.applications();
            let app = apps.iter().find(|a| a.process_id() == pid).ok_or_else(|| {
                CodecError::Audio(format!("no running application with pid {pid}"))
            })?;
            (
                SCContentFilter::create()
                    .with_display(&display)
                    .with_including_applications(&[app], &[])
                    .build(),
                AudioSourceInfo {
                    source_id: request.source_id.clone(),
                    kind: AudioSourceKind::Application,
                    name: app.application_name(),
                    pid: Some(pid as u32),
                    app_id: Some(app.bundle_identifier()),
                    available: true,
                    limitation: None,
                    desktop_fallback: false,
                },
            )
        };
        let format = AudioFormat {
            sample_rate: request.format.sample_rate,
            channels: request.format.channels,
        };
        let mut config = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_minimum_frame_interval(&CMTime::new(1, 1))
            .with_captures_audio(true)
            .with_sample_rate(format.sample_rate as i32)
            .with_channel_count(i32::from(format.channels));
        config.set_excludes_current_process_audio(true);
        let mut stream = SCStream::new(&filter, &config);
        let handler_sink = sink.clone();
        stream.add_output_handler(
            move |sample: CMSampleBuffer, of_type: SCStreamOutputType| {
                if of_type != SCStreamOutputType::Audio {
                    return;
                }
                let Some(list) = sample.audio_buffer_list() else {
                    return;
                };
                let per_plane = list.get(0).map_or(1, |b| b.number_channels);
                let planes: Vec<&[u8]> = list.iter().map(|b| b.data()).collect();
                let samples = interleave_f32(&planes, per_plane, format.channels);
                if samples.is_empty() {
                    return;
                }
                let frames = samples.len() / usize::from(format.channels);
                handler_sink.on_audio(AudioFrame {
                    pts_us: crate::types::media_clock_us()
                        .saturating_sub(format.duration_us(frames)),
                    format,
                    samples,
                });
            },
            SCStreamOutputType::Audio,
        );
        stream
            .start_capture()
            .map_err(|e| CodecError::Audio(format!("SCStream start: {e:?}")))?;
        sink.on_state(AudioCaptureState::Active);
        Ok(Box::new(SckStream {
            source,
            stream: Some(stream),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_float_is_interleaved() {
        let l: Vec<u8> = [0.5f32, -0.5]
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        let r: Vec<u8> = [1.0f32, 2.0].iter().flat_map(|f| f.to_ne_bytes()).collect();
        assert_eq!(
            interleave_f32(&[&l, &r], 1, 2),
            vec![16384, 32767, -16384, 32767]
        );
        // Stereo interleaved to mono takes the first channel.
        let i: Vec<u8> = [0.25f32, 0.75, -1.0, 0.0]
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        assert_eq!(interleave_f32(&[&i], 2, 1), vec![8192, -32767]);
    }

    /// Captures real host audio: only with explicit consent, never in CI on
    /// a personal machine. `CUA_ENV_ALLOW_HOST_APP_EFFECTS=1`.
    #[test]
    #[ignore = "captures real host audio; requires TCC and CUA_ENV_ALLOW_HOST_APP_EFFECTS=1"]
    fn sck_desktop_capture_delivers_audio() {
        if std::env::var("CUA_ENV_ALLOW_HOST_APP_EFFECTS").as_deref() != Ok("1") {
            eprintln!("skipped: set CUA_ENV_ALLOW_HOST_APP_EFFECTS=1");
            return;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<usize>(64);
        let sink: Arc<dyn AudioFrameSink> = Arc::new(move |f: AudioFrame| {
            let _ = tx.try_send(f.samples.len());
        });
        let mut stream = SckCapture::new()
            .start(&AudioCaptureRequest::desktop(), sink)
            .unwrap();
        let got = rx.recv_timeout(std::time::Duration::from_secs(5));
        stream.stop();
        assert!(got.is_ok(), "no audio callback within 5 s");
    }
}
