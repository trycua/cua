// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `audio`: the sound server and its devices are the ones the image
//! claims; spacesd's desktop audio track carries the tone fixture as Opus
//! that decodes to the tone; the uplink route (virtual mic) loops a 660 Hz
//! tone back; the A/V sync fixture's flash and beep land within budget.

use std::time::{Duration, Instant};

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, Command, MediaOptions};
use futures_util::StreamExt as _;

use super::desktop;
use crate::{Ctx, Recorder};

/// RMS of interleaved s16 samples in dBFS (`None` for silence).
pub fn rms_dbfs(samples: &[i16]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64 / 32768.0).powi(2)).sum();
    let rms = (sum / samples.len() as f64).sqrt();
    (rms > 0.0).then(|| 20.0 * rms.log10())
}

/// Goertzel power of `freq` in mono samples at `rate`.
pub fn tone_power(samples: &[f64], rate: f64, freq: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * freq / rate;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for x in samples {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Whether `freq` dominates `other` frequencies in the samples.
pub fn dominant(samples: &[f64], rate: f64, freq: f64, others: &[f64]) -> bool {
    let p = tone_power(samples, rate, freq);
    others
        .iter()
        .all(|f| p > 4.0 * tone_power(samples, rate, *f))
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("audio") {
        return;
    }
    let claims: &[&str] = &["feature:audio.desktop"];
    if ctx.os() == "linux" {
        let audio = ctx.manifest.manifest.audio.clone();
        rec.run("audio.backend", claims, Duration::from_secs(15), async {
            match ctx
                .client
                .run(
                    Command::new("pactl")
                        .arg("info")
                        .timeout(Duration::from_secs(10)),
                )
                .await
            {
                Ok(out) if out.success() => {
                    let text = out.stdout_str();
                    let field = |name: &str| {
                        text.lines()
                            .find_map(|l| {
                                l.strip_prefix(&format!("{name}: "))
                                    .map(str::trim)
                                    .map(str::to_owned)
                            })
                            .unwrap_or_default()
                    };
                    let server = field("Server Name");
                    let (sink, source) = (field("Default Sink"), field("Default Source"));
                    ctx.fidelity.lock().await.audio_backend = if server.contains("PipeWire") {
                        "pipewire".into()
                    } else {
                        server.to_lowercase()
                    };
                    let sink_ok = audio.default_sink.is_empty() || sink == audio.default_sink;
                    let source_ok =
                        audio.default_source.is_empty() || source == audio.default_source;
                    Check::new(
                        "audio.backend",
                        super::verdict(!server.is_empty() && sink_ok && source_ok),
                        format!(
                            "{server}; default sink {sink} (want {}), source {source} (want {})",
                            audio.default_sink, audio.default_source
                        ),
                    )
                }
                Ok(out) => Check::new(
                    "audio.backend",
                    Status::Fail,
                    format!(
                        "pactl info exited {:?}: {}",
                        out.status.code,
                        out.stderr_str().trim()
                    ),
                ),
                Err(error) => Check::new("audio.backend", Status::Fail, format!("pactl: {error}")),
            }
        })
        .await;
    }

    if !ctx.supports("audio.desktop") {
        rec.push(
            Check::new(
                "audio.desktop_capture",
                Status::Fail,
                format!(
                    "audio.desktop unsupported: {}",
                    ctx.limitation("audio.desktop")
                ),
            ),
            claims,
        )
        .await;
        return;
    }
    let has_tone = ctx.manifest.manifest.fixtures.has("tone");
    if !has_tone {
        return;
    }
    rec.run_effectful(
        "audio.desktop_capture",
        &["feature:audio.desktop", "feature:audio.opus"],
        Duration::from_secs(40),
        async {
            if let Err(error) = desktop::start_fixture(ctx, "tone").await {
                return Check::new("audio.desktop_capture", Status::Fail, error);
            }
            capture_tone(ctx).await
        },
    )
    .await;

    let audio = ctx.manifest.manifest.audio.clone();
    let root = ctx.manifest.manifest.fixtures.root.clone();
    if !audio.uplink_input_sink.is_empty() && !audio.default_source.is_empty() {
        rec.run_effectful("audio.uplink_route", &["feature:audio.uplink"], Duration::from_secs(40), async {
            // 660 Hz into the uplink sink must come out of the virtual mic.
            // Under emulation (CUA_DOCTOR_TIMEOUT_SCALE) the probe starts
            // later, so the tone plays longer and the head start grows; the
            // measurement (2 s, 660 Hz dominant) is the same. The tone is one
            // computed second, repeated (pure Python is slow under TCG).
            let scale = crate::timeout_scale();
            let script = format!(
                "python3 -c \"import math,struct,wave\nw=wave.open('/tmp/cua-doctor-{n}.wav','wb');w.setnchannels(1);w.setsampwidth(2);w.setframerate(48000)\ns=b''.join(struct.pack('<h',int(12000*math.sin(2*math.pi*660*i/48000))) for i in range(48000))\nw.writeframes(s*{tone})\" \
                 && (pacat --playback --device={sink} --file-format=wav /tmp/cua-doctor-{n}.wav & sleep {lead}; \
                 python3 {root}/audio_probe.py --source {source} --seconds 2 --freq 660 --freq 440; wait; rm -f /tmp/cua-doctor-{n}.wav)",
                n = ctx.nonce,
                sink = audio.uplink_input_sink,
                source = audio.default_source,
                // Whole seconds: one second of 660 Hz (660 whole cycles)
                // repeats seamlessly, so only 48000 samples are computed.
                tone = (4.0 * scale).ceil() as u32,
                lead = 0.5 * scale,
            );
            match ctx.client.run(Command::new("/bin/sh").args(["-c", &script]).timeout(crate::scaled(Duration::from_secs(30)))).await {
                Ok(out) => {
                    let last = out.stdout_str().lines().last().unwrap_or_default().to_owned();
                    let probe: serde_json::Value = serde_json::from_str(&last).unwrap_or_default();
                    let fraction = probe["tones"]["660"]["dominant_fraction"].as_f64().unwrap_or(0.0);
                    Check::new(
                        "audio.uplink_route",
                        super::verdict(fraction >= 0.8 && probe["rms_dbfs"].is_number()),
                        format!("660 Hz into {} heard on {} (dominant fraction {fraction:.2}, rms {})", audio.uplink_input_sink, audio.default_source, probe["rms_dbfs"]),
                    )
                }
                Err(error) => Check::new("audio.uplink_route", Status::Fail, error.to_string()),
            }
        })
        .await;
    }

    if ctx.manifest.manifest.fixtures.has("avsync") {
        // The manifest's budget, stretched only under emulation
        // (CUA_DOCTOR_TIMEOUT_SCALE > 1); accelerated runs keep it exact.
        let budget = (f64::from(ctx.manifest.manifest.av_skew_budget(&ctx.runtime))
            * crate::timeout_scale())
        .round() as u32;
        rec.run_effectful(
            "audio.avsync",
            &["feature:audio.desktop"],
            Duration::from_secs(40),
            async {
                let fixture = match desktop::start_fixture(ctx, "avsync").await {
                    Ok(f) => f,
                    Err(error) => return Check::new("audio.avsync", Status::Fail, error),
                };
                // At least two complete flash/beep pairs, waited for as long
                // as the run is scaled (bounded: one read per second). Under
                // emulation a flash can land well before its beep, so count
                // pairs, not flashes.
                let deadline = std::time::Instant::now() + crate::scaled(Duration::from_secs(4));
                tokio::time::sleep(Duration::from_secs(4)).await;
                let mut skews = av_skews(&desktop::events(ctx, &fixture).await);
                while std::time::Instant::now() < deadline && skews.len() < 2 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    skews = av_skews(&desktop::events(ctx, &fixture).await);
                }
                let ok = skews.len() >= 2 && skews.iter().all(|s| s.abs() <= budget as f64);
                Check::new(
                    "audio.avsync",
                    super::verdict(ok),
                    format!(
                        "{} flash/beep pairs, skews {skews:?} ms (budget {budget} ms on {})",
                        skews.len(),
                        ctx.runtime
                    ),
                )
                .fact("budget_ms", budget)
            },
        )
        .await;
    }
}

/// Opens an audio-only media session and checks the decoded desktop mix
/// carries the tone fixture (440/880 Hz). Bounded: 25 s, 5000 messages.
async fn capture_tone(ctx: &Ctx) -> Check {
    let id = "audio.desktop_capture";
    let session = match ctx
        .client
        .open_media(MediaOptions {
            disable_video: true,
            audio: Some(pb::AudioOptions {
                enabled: true,
                encoding: Some(pb::AudioEncoding {
                    codecs: vec![pb::AudioCodec::Opus as i32],
                    dtx: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    {
        Ok(s) => s,
        Err(error) => return Check::new(id, Status::Fail, format!("OpenMedia(audio): {error}")),
    };
    let config = cua_media_codec::audio::codec::AudioEncodingConfig::downlink();
    let mut decoder = match cua_media_codec::audio::codec::open_audio_decoder(&config) {
        Ok(d) => d,
        Err(error) => return Check::new(id, Status::Fail, format!("Opus decoder: {error}")),
    };
    let channels = usize::from(config.format.channels.max(1));
    let rate = f64::from(config.format.sample_rate);
    let started = Instant::now();
    let mut pcm: Vec<i16> = Vec::new();
    let mut packets = 0usize;
    let result = async {
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(session.ws_url.as_str()),
        )
        .await
        .map_err(|_| "media WebSocket connect timed out".to_owned())?
        .map_err(|e| format!("media WebSocket: {e}"))?;
        let deadline = started + Duration::from_secs(20);
        for _ in 0..5000 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(Some(Ok(message))) = tokio::time::timeout(remaining, socket.next()).await else {
                break;
            };
            let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
                continue;
            };
            let Ok((_, payload)) =
                cua_media_codec::audio::packet::AudioPacketHeader::decode(&bytes)
            else {
                continue;
            };
            packets += 1;
            if let Ok(samples) = decoder.decode(payload) {
                pcm.extend(samples);
            }
            // 2 s of audio is enough.
            if pcm.len() >= 2 * 48_000 * channels {
                break;
            }
        }
        let _ = socket.close(None).await;
        Ok::<_, String>(())
    }
    .await;
    let _ = ctx
        .client
        .stream()
        .close_media(pb::CloseMediaRequest {
            media_session_id: session.session_id.clone(),
        })
        .await;
    if let Err(error) = result {
        return Check::new(id, Status::Fail, error);
    }
    let mono: Vec<f64> = pcm
        .chunks(channels)
        .map(|c| c.iter().map(|s| *s as f64).sum::<f64>() / channels as f64)
        .collect();
    let rms = rms_dbfs(&pcm);
    // The fixture alternates 440 and 880 Hz with gaps; either dominating
    // an unrelated 660 Hz shows the tone came through.
    let heard = dominant(&mono, rate, 440.0, &[660.0]) || dominant(&mono, rate, 880.0, &[660.0]);
    Check::new(
        id,
        super::verdict(packets > 10 && rms.is_some_and(|r| r > -40.0) && heard),
        format!(
            "{packets} Opus packets, {:.1} s decoded, rms {}, tone {}",
            mono.len() as f64 / rate,
            rms.map(|r| format!("{r:.1} dBFS"))
                .unwrap_or("silent".into()),
            if heard { "heard" } else { "NOT heard" }
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin() * 10_000.0)
            .collect()
    }

    #[test]
    fn goertzel_finds_the_tone() {
        let s = sine(440.0, 48_000);
        assert!(dominant(&s, 48_000.0, 440.0, &[660.0, 880.0]));
        assert!(!dominant(&s, 48_000.0, 660.0, &[440.0]));
    }

    #[test]
    fn rms_levels() {
        assert_eq!(rms_dbfs(&[]), None);
        assert_eq!(rms_dbfs(&[0, 0]), None);
        let full: Vec<i16> = vec![i16::MAX; 10];
        assert!(rms_dbfs(&full).unwrap() > -0.1);
        let quiet: Vec<i16> = vec![33; 10];
        assert!(rms_dbfs(&quiet).unwrap() < -59.0);
    }
}

/// Flash-minus-beep skews in ms (0.1 ms precision), one per boundary that
/// has both events.
fn av_skews(events: &[serde_json::Value]) -> Vec<f64> {
    let mut pairs: std::collections::BTreeMap<String, (Option<f64>, Option<f64>)> =
        Default::default();
    for e in events {
        let Some(boundary) = e.get("boundary").map(|b| b.to_string()) else {
            continue;
        };
        let mono = e["mono"].as_f64();
        match e["type"].as_str() {
            Some("beep") => pairs.entry(boundary).or_default().0 = mono,
            Some("flash_drawn") => pairs.entry(boundary).or_default().1 = mono,
            _ => {}
        }
    }
    pairs
        .values()
        .filter_map(|(b, f)| Some(((f.as_ref()? - b.as_ref()?) * 1000.0 * 10.0).round() / 10.0))
        .collect()
}

#[cfg(test)]
mod av_tests {
    use super::av_skews;
    use serde_json::json;

    #[test]
    fn only_complete_pairs_count() {
        let events = vec![
            json!({"type": "beep", "boundary": 1, "mono": 1.000}),
            json!({"type": "flash_drawn", "boundary": 1, "mono": 1.020}),
            json!({"type": "flash_drawn", "boundary": 2, "mono": 2.010}),
        ];
        assert_eq!(av_skews(&events), vec![20.0]);
    }
}
