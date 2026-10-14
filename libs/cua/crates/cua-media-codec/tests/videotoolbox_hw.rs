// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in: which VideoToolbox H.264 sessions run on the media engine.
//!
//! `CUA_CODEC_TEST_VIDEOTOOLBOX_HW=1 cargo test -p cua-media-codec --test videotoolbox_hw -- --nocapture`
//!
//! Hardware depends on the Mac (and is absent in most macOS VMs), so the
//! test is skipped unless asked for. It prints, per size, whether the
//! encoder and the decoder session report hardware
//! (`UsingHardwareAcceleratedVideoEncoder` / `...Decoder`), and asserts that
//! a Mac with a hardware H.264 decoder uses it for a real stream.
//! `CUA_CODEC_TEST_EXPECT_HW_ENCODE=1` also asserts a hardware encoder
//! session at every size (Apple silicon, not in a VM). Memory is
//! bounded: at most 1920x1080 BGRA, 10 frames per size.
#![cfg(target_os = "macos")]

use cua_media_codec::backends::videotoolbox::{hardware_decode_supported, VtDecoder, VtEncoder};
use cua_media_codec::video::{VideoDecoder, VideoEncoder};
use cua_media_codec::*;

const FRAMES: u64 = 10;

fn gated() -> bool {
    let on = std::env::var_os("CUA_CODEC_TEST_VIDEOTOOLBOX_HW").is_some();
    if !on {
        eprintln!("skipped: set CUA_CODEC_TEST_VIDEOTOOLBOX_HW=1");
    }
    on
}

/// Encodes a moving gradient and decodes it; returns (encoder hw, decoder hw, decoded frames).
fn run(w: u32, h: u32) -> (bool, Option<bool>, u64) {
    let cfg = EncoderConfig::new(VideoCodec::H264, w, h, 30).with_bitrate_kbps(4000);
    let mut enc = VtEncoder::new(&cfg).expect("VideoToolbox encoder");
    let mut dec = VtDecoder::new(VideoCodec::H264).expect("VideoToolbox decoder");
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut decoded = 0;
    for i in 0..FRAMES {
        for (n, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = (n as u32 % w) as u64;
            p[0] = (x + i * 8) as u8;
            p[1] = (n as u64 / w as u64 + i * 4) as u8;
            p[2] = 0x80;
            p[3] = 0xff;
        }
        for au in enc
            .encode(&VideoFrame::bgra(w, h, i * 33_333, &px))
            .expect("encode")
        {
            if dec.decode(&au.data, au.pts_us).expect("decode").is_some() {
                decoded += 1;
            }
        }
    }
    (enc.is_hardware(), dec.is_hardware(), decoded)
}

#[test]
fn videotoolbox_h264_hardware_report() {
    if !gated() {
        return;
    }
    let hw_decode = hardware_decode_supported(VideoCodec::H264);
    eprintln!("VTIsHardwareDecodeSupported(H.264) = {hw_decode}");
    let expect_hw_encode = std::env::var_os("CUA_CODEC_TEST_EXPECT_HW_ENCODE").is_some();
    let mut decoder_hw_at_1080 = None;
    for (w, h) in [(320, 240), (640, 360), (1280, 720), (1920, 1080)] {
        let (enc_hw, dec_hw, decoded) = run(w, h);
        eprintln!("{w}x{h}: encoder hardware={enc_hw} decoder hardware={dec_hw:?} decoded={decoded}/{FRAMES}");
        assert!(
            decoded >= FRAMES - 1,
            "{w}x{h}: decoded {decoded} of {FRAMES}"
        );
        if expect_hw_encode {
            assert!(enc_hw, "{w}x{h}: the encoder session is software");
        }
        if (w, h) == (1920, 1080) {
            decoder_hw_at_1080 = dec_hw;
        }
    }
    if hw_decode {
        assert_eq!(
            decoder_hw_at_1080,
            Some(true),
            "hardware decode is supported but the 1080p session is software"
        );
    }
}
