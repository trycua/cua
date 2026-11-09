// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Video conformance: encode -> decode round trips with PSNR thresholds,
//! keyframe/epoch behaviour, bitrate adherence and runtime fallback.
//!
//! Hermetic: pure encode/decode in memory; no apps, devices or network.
//! Sizes are bounded (<= 640x360, <= 90 frames) to keep memory small.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use common::*;
use cua_media_codec::backends::{open_decoder, open_encoder, EncoderFactory};
use cua_media_codec::bitstream::has_parameter_sets;
use cua_media_codec::probe::{EncoderInfo, ProbeStatus};
use cua_media_codec::select::{
    select, ClientDecoder, DecoderKind, FallbackEncoder, FallbackPolicy, SelectionPrefs,
};
use cua_media_codec::*;

const W: u32 = 640;
const H: u32 = 360;

/// Encodes `frames` and decodes them with `decoder`, returning the minimum
/// and mean luma PSNR over decoded pictures.
fn round_trip(
    enc_backend: Backend,
    dec_backend: Backend,
    codec: VideoCodec,
    frames: &[Vec<u8>],
    kbps: u32,
) -> (f64, f64) {
    let cfg = EncoderConfig::new(codec, W, H, 30).with_bitrate_kbps(kbps);
    let mut enc = open_encoder(enc_backend, &cfg).expect("encoder");
    let mut dec = open_decoder(dec_backend, codec).expect("decoder");
    let mut psnrs = Vec::new();
    for (i, px) in frames.iter().enumerate() {
        let aus = enc
            .encode(&VideoFrame::bgra(W, H, i as u64 * 33_333, px))
            .expect("encode");
        for au in aus {
            if let Some(pic) = dec.decode(&au.data, au.pts_us).expect("decode") {
                assert_eq!((pic.width, pic.height), (W, H));
                // Measure codec loss in the coded domain (Y plane against the
                // source converted to I420), so 4:2:0 subsampling of
                // saturated edges does not count against the encoder.
                let src = cua_media_codec::convert::bgra_to_i420(W, H, px, W as usize * 4);
                psnrs.push(cua_media_codec::convert::psnr(&src.y, &pic.y));
            }
        }
    }
    assert!(
        psnrs.len() >= frames.len() - 1,
        "decoded {} of {} frames",
        psnrs.len(),
        frames.len()
    );
    let min = psnrs.iter().cloned().fold(f64::INFINITY, f64::min);
    let mean = psnrs.iter().sum::<f64>() / psnrs.len() as f64;
    eprintln!("{enc_backend:?}->{dec_backend:?} {codec:?}: min {min:.1} dB, mean {mean:.1} dB over {} frames", psnrs.len());
    (min, mean)
}

fn content_sets() -> Vec<(&'static str, Vec<Vec<u8>>, f64)> {
    vec![
        (
            "color grid",
            (0..10).map(|_| color_grid(W, H)).collect(),
            35.0,
        ),
        (
            "text-like",
            (0..10).map(|i| text_like(W, H, 1 + i / 5)).collect(),
            28.0,
        ),
        ("motion", (0..30).map(|i| motion(W, H, i)).collect(), 30.0),
    ]
}

#[test]
fn openh264_round_trip_meets_psnr_thresholds() {
    for (name, frames, threshold) in content_sets() {
        let (min, _) = round_trip(
            Backend::OpenH264,
            Backend::OpenH264,
            VideoCodec::H264,
            &frames,
            3000,
        );
        assert!(min >= threshold, "{name}: min PSNR {min:.1} < {threshold}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn videotoolbox_round_trips_meet_psnr_thresholds() {
    for (name, frames, threshold) in content_sets() {
        let (min, _) = round_trip(
            Backend::VideoToolbox,
            Backend::VideoToolbox,
            VideoCodec::H264,
            &frames,
            3000,
        );
        assert!(min >= threshold, "VT h264 {name}: {min:.1} < {threshold}");
        // Cross-implementation: VideoToolbox bitstream into OpenH264.
        let (min, _) = round_trip(
            Backend::VideoToolbox,
            Backend::OpenH264,
            VideoCodec::H264,
            &frames,
            3000,
        );
        assert!(
            min >= threshold,
            "VT->OpenH264 {name}: {min:.1} < {threshold}"
        );
        let (min, _) = round_trip(
            Backend::VideoToolbox,
            Backend::VideoToolbox,
            VideoCodec::Hevc,
            &frames,
            3000,
        );
        assert!(min >= threshold, "VT hevc {name}: {min:.1} < {threshold}");
    }
    // OpenH264 bitstream into the VideoToolbox decoder.
    let frames: Vec<Vec<u8>> = (0..10).map(|i| motion(W, H, i)).collect();
    let (min, _) = round_trip(
        Backend::OpenH264,
        Backend::VideoToolbox,
        VideoCodec::H264,
        &frames,
        3000,
    );
    assert!(min >= 30.0, "OpenH264->VT {min:.1}");
}

fn keyframe_and_epoch_behaviour(backend: Backend) {
    let cfg = EncoderConfig::new(VideoCodec::H264, 320, 240, 30).with_bitrate_kbps(800);
    let mut enc = open_encoder(backend, &cfg).unwrap();
    let encode = |enc: &mut Box<dyn VideoEncoder>, w: u32, h: u32, i: u32| {
        let px = motion(w, h, i);
        enc.encode(&VideoFrame::bgra(w, h, u64::from(i) * 33_333, &px))
            .unwrap()
            .pop()
            .expect("one AU per frame")
    };
    let first = encode(&mut enc, 320, 240, 0);
    assert!(first.keyframe && first.codec_epoch == 1);
    assert!(
        has_parameter_sets(VideoCodec::H264, &first.data),
        "first AU lacks SPS/PPS"
    );
    for i in 1..10 {
        let au = encode(&mut enc, 320, 240, i);
        assert!(
            !au.keyframe,
            "{backend:?}: unexpected keyframe at {i} (IDR-on-demand)"
        );
    }
    enc.force_keyframe();
    let forced = encode(&mut enc, 320, 240, 10);
    assert!(forced.keyframe && has_parameter_sets(VideoCodec::H264, &forced.data));
    assert_eq!(forced.codec_epoch, 1);
    // In-place bitrate change keeps the epoch.
    let mut next = enc.config().clone();
    next.bitrate_kbps = 400;
    assert_eq!(enc.reconfigure(&next).unwrap(), Reconfigured::InPlace);
    assert_eq!(encode(&mut enc, 320, 240, 11).codec_epoch, 1);
    // A size change restarts: new epoch, keyframe with parameter sets.
    let resized = encode(&mut enc, 160, 120, 12);
    assert!(resized.keyframe && has_parameter_sets(VideoCodec::H264, &resized.data));
    assert_eq!((resized.codec_epoch, resized.width), (2, 160));
}

#[test]
fn openh264_keyframes_and_epochs() {
    keyframe_and_epoch_behaviour(Backend::OpenH264);
}

#[cfg(target_os = "macos")]
#[test]
fn videotoolbox_keyframes_and_epochs() {
    keyframe_and_epoch_behaviour(Backend::VideoToolbox);
}

fn bitrate_adherence(backend: Backend) {
    let target_kbps = 1000;
    let fps = 30;
    let frames = 90u32;
    let cfg = EncoderConfig::new(VideoCodec::H264, W, H, fps).with_bitrate_kbps(target_kbps);
    let mut enc = open_encoder(backend, &cfg).unwrap();
    let mut bytes = 0usize;
    for i in 0..frames {
        let px = motion(W, H, i);
        for au in enc
            .encode(&VideoFrame::bgra(
                W,
                H,
                u64::from(i) * 1_000_000 / u64::from(fps),
                &px,
            ))
            .unwrap()
        {
            // Exclude the first IDR, as a rate controller does.
            if i > 0 {
                bytes += au.data.len();
            }
        }
    }
    let seconds = f64::from(frames - 1) / f64::from(fps);
    let kbps = bytes as f64 * 8.0 / seconds / 1000.0;
    eprintln!("{backend:?}: target {target_kbps} kbps, measured {kbps:.0} kbps");
    assert!(
        kbps > f64::from(target_kbps) * 0.5 && kbps < f64::from(target_kbps) * 1.5,
        "{backend:?} bitrate {kbps:.0} kbps outside 50-150% of {target_kbps}"
    );
}

#[test]
fn openh264_bitrate_adherence() {
    bitrate_adherence(Backend::OpenH264);
}

#[cfg(target_os = "macos")]
#[test]
fn videotoolbox_bitrate_adherence() {
    bitrate_adherence(Backend::VideoToolbox);
}

// ---- fallback on injected runtime error

/// A fake hardware encoder that works for `ok_frames` frames, then fails
/// like a lost GPU session.
struct Flaky {
    cfg: EncoderConfig,
    left: Arc<AtomicU32>,
    inner: Box<dyn VideoEncoder>,
}

impl VideoEncoder for Flaky {
    fn backend(&self) -> Backend {
        Backend::Fake
    }
    fn config(&self) -> &EncoderConfig {
        &self.cfg
    }
    fn codec_epoch(&self) -> u64 {
        self.inner.codec_epoch()
    }
    fn reconfigure(&mut self, c: &EncoderConfig) -> Result<Reconfigured> {
        self.inner.reconfigure(c)
    }
    fn force_keyframe(&mut self) {
        self.inner.force_keyframe()
    }
    fn encode(&mut self, f: &VideoFrame<'_>) -> Result<Vec<AccessUnit>> {
        if self.left.fetch_sub(1, Ordering::SeqCst) == 0 {
            return Err(CodecError::backend(Backend::Fake, "injected: device lost"));
        }
        self.inner.encode(f)
    }
}

struct Factory {
    left: Arc<AtomicU32>,
}

impl EncoderFactory for Factory {
    fn open(&self, backend: Backend, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>> {
        match backend {
            Backend::Fake => Ok(Box::new(Flaky {
                cfg: config.clone(),
                left: self.left.clone(),
                inner: open_encoder(Backend::OpenH264, config)?,
            })),
            Backend::Nvenc => Err(CodecError::unavailable(
                Backend::Nvenc,
                "injected: open failure",
            )),
            other => open_encoder(other, config),
        }
    }
}

fn usable(backend: Backend, class: LatencyClass) -> EncoderInfo {
    EncoderInfo {
        backend,
        codecs: vec![VideoCodec::H264],
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

#[test]
fn fallback_engages_on_injected_runtime_error_with_new_epoch_and_keyframe() {
    // Ranking: Nvenc (fails to open) -> Fake (fails after 5 frames) -> OpenH264.
    let encoders = [
        usable(Backend::Nvenc, LatencyClass::Hardware),
        usable(Backend::Fake, LatencyClass::Hardware),
        usable(Backend::OpenH264, LatencyClass::SoftwareRealtime),
    ];
    let client = [ClientDecoder::new(
        DecoderKind::OpenH264,
        &[VideoCodec::H264],
        false,
    )];
    let mut choice = select(&encoders, &client, &SelectionPrefs::default()).unwrap();
    // Fake sorts after Nvenc by priority; keep that order.
    choice
        .candidates
        .sort_by_key(|c| c.backend != Backend::Nvenc);
    assert_eq!(choice.best().backend, Backend::Nvenc);
    let cfg = EncoderConfig::new(VideoCodec::H264, 320, 240, 30).with_bitrate_kbps(800);
    let mut enc = FallbackEncoder::with_factory(
        &choice,
        &cfg,
        Box::new(Factory {
            left: Arc::new(AtomicU32::new(5)),
        }),
        FallbackPolicy::default(),
    )
    .unwrap();
    assert_eq!(
        enc.backend(),
        Backend::Fake,
        "open failure must skip to the next candidate"
    );
    let mut dec = open_decoder(Backend::OpenH264, VideoCodec::H264).unwrap();
    let mut epochs = Vec::new();
    for i in 0..12u32 {
        let px = motion(320, 240, i);
        let aus = enc
            .encode(&VideoFrame::bgra(320, 240, u64::from(i) * 33_333, &px))
            .expect("fallback hides the failure");
        assert_eq!(aus.len(), 1);
        let au = &aus[0];
        if epochs.last() != Some(&au.codec_epoch) {
            assert!(
                au.keyframe,
                "first AU of epoch {} must be a keyframe",
                au.codec_epoch
            );
            dec.reset().unwrap();
            epochs.push(au.codec_epoch);
        }
        assert!(
            dec.decode(&au.data, au.pts_us).unwrap().is_some(),
            "frame {i} undecodable"
        );
    }
    assert_eq!(enc.backend(), Backend::OpenH264);
    assert_eq!(epochs, vec![1, 2], "one switch, monotonic epochs");
    assert_eq!(enc.failed.len(), 2, "{:?}", enc.failed);
}

#[test]
fn probe_reports_software_encoder_and_real_hardware_state() {
    let infos = cua_media_codec::probe::probe_with(&cua_media_codec::probe::Isolation::InProcess);
    for i in &infos {
        eprintln!(
            "{:?}: {:?} codecs={:?} hw={} {:?} ({} ms)",
            i.backend, i.status, i.codecs, i.hardware, i.device, i.probe_ms
        );
    }
    let oh = infos
        .iter()
        .find(|i| i.backend == Backend::OpenH264)
        .unwrap();
    assert!(oh.is_usable());
    #[cfg(target_os = "macos")]
    {
        let vt = infos
            .iter()
            .find(|i| i.backend == Backend::VideoToolbox)
            .unwrap();
        assert!(vt.is_usable() && vt.codecs.contains(&VideoCodec::H264));
    }
    // The hardware probes exist only with their features (`hw`, `server`);
    // the default build is software-only.
    #[cfg(all(target_os = "linux", feature = "vaapi", feature = "nvenc"))]
    {
        // No GPU in CI/docker: VA-API and NVENC must be cleanly unavailable.
        for b in [Backend::Vaapi, Backend::Nvenc] {
            let i = infos.iter().find(|i| i.backend == b).unwrap();
            if std::env::var_os("CUA_CODEC_TEST_VAAPI").is_none()
                && std::env::var_os("CUA_CODEC_TEST_NVENC").is_none()
            {
                assert!(
                    matches!(i.status, ProbeStatus::Unavailable { .. }),
                    "{b:?}: {:?}",
                    i.status
                );
            }
        }
    }
    // Selecting for a WebCodecs H.264 client works on every host.
    let client = [ClientDecoder::new(
        DecoderKind::WebCodecs,
        &[VideoCodec::H264],
        true,
    )];
    let choice = select(&infos, &client, &SelectionPrefs::default()).unwrap();
    eprintln!("selected: {:?}", choice.best());
}

#[test]
fn isolated_probe_through_the_probe_binary() {
    use cua_media_codec::probe::{probe_in_child, ProbeStatus};
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_cua-codec-probe"));
    let info = probe_in_child(
        Backend::OpenH264,
        &exe,
        &[],
        std::time::Duration::from_secs(60),
    );
    assert_eq!(info.status, ProbeStatus::Available, "{info:?}");
    let info = probe_in_child(
        Backend::Nvenc,
        &exe,
        &[],
        std::time::Duration::from_secs(60),
    );
    assert!(!info.is_usable());
}
