// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Encode latency per backend at 720p / 1080p (P-frames of moving content),
//! plus BGRA->I420 conversion and Opus 20 ms frames.
//!
//! Bounded: 8 pre-rendered frames per resolution (~66 MiB at 1080p), small
//! sample counts. Run: `cargo bench -p cua-media-codec --bench encode`.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use cua_media_codec::backends::open_encoder;
use cua_media_codec::probe::{probe_with, Isolation};
use cua_media_codec::{EncoderConfig, VideoCodec, VideoFrame};

fn frames(w: u32, h: u32) -> Vec<Vec<u8>> {
    (0..8u32)
        .map(|f| {
            let mut v = vec![255u8; (w * h * 4) as usize];
            let size = h / 4;
            let (bx, by) = ((f * 37) % (w - size), (f * 17) % (h - size));
            for y in 0..h {
                for x in 0..w {
                    let o = ((y * w + x) * 4) as usize;
                    if x >= bx && x < bx + size && y >= by && y < by + size {
                        v[o..o + 3].copy_from_slice(&[40, 200, 240]);
                    } else {
                        v[o] = (x * 255 / w) as u8;
                        v[o + 1] = (y * 255 / h) as u8;
                        v[o + 2] = ((x / 8 + y / 8 + f) % 2 * 200) as u8;
                    }
                }
            }
            v
        })
        .collect()
}

fn bench_video(c: &mut Criterion) {
    let usable: Vec<_> = probe_with(&Isolation::InProcess)
        .into_iter()
        .filter(|i| i.is_usable() && i.codecs.contains(&VideoCodec::H264))
        .collect();
    let mut group = c.benchmark_group("encode_h264");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(4))
        .warm_up_time(Duration::from_secs(1));
    for (w, h, label) in [(1280u32, 720u32, "720p"), (1920, 1080, "1080p")] {
        let input = frames(w, h);
        for info in &usable {
            let cfg = EncoderConfig::new(VideoCodec::H264, w, h, 30);
            let mut enc = open_encoder(info.backend, &cfg).expect("encoder");
            let mut i = 0u64;
            // Prime with the IDR so the loop measures steady-state P-frames.
            enc.encode(&VideoFrame::bgra(w, h, 0, &input[0])).unwrap();
            group.bench_function(
                BenchmarkId::new(format!("{:?}", info.backend), label),
                |b| {
                    b.iter(|| {
                        i += 1;
                        let px = &input[(i % 8) as usize];
                        enc.encode(&VideoFrame::bgra(w, h, i * 33_333, px)).unwrap()
                    })
                },
            );
        }
        group.bench_function(BenchmarkId::new("bgra_to_i420", label), |b| {
            let mut out = cua_media_codec::convert::I420Buffer::new(w, h);
            b.iter(|| {
                cua_media_codec::convert::bgra_to_i420_into(
                    w,
                    h,
                    &input[1],
                    w as usize * 4,
                    &mut out,
                )
            })
        });
    }
    group.finish();
}

#[cfg(feature = "opus")]
fn bench_opus(c: &mut Criterion) {
    use cua_media_codec::audio::codec::{
        open_audio_decoder, open_audio_encoder, AudioEncodingConfig,
    };
    let cfg = AudioEncodingConfig::downlink();
    let mut enc = open_audio_encoder(&cfg).unwrap();
    let mut dec = open_audio_decoder(&cfg).unwrap();
    let pcm: Vec<i16> = (0..1920)
        .map(|n| ((n as f64 * 0.13).sin() * 12000.0) as i16)
        .collect();
    let pkt = enc.encode(&pcm).unwrap().data;
    let mut group = c.benchmark_group("opus_48k_stereo_20ms");
    group.sample_size(50);
    group.bench_function("encode", |b| b.iter(|| enc.encode(&pcm).unwrap()));
    group.bench_function("decode", |b| b.iter(|| dec.decode(&pkt).unwrap()));
    group.finish();
}

#[cfg(not(feature = "opus"))]
fn bench_opus(_c: &mut Criterion) {}

criterion_group!(benches, bench_video, bench_opus);
criterion_main!(benches);
