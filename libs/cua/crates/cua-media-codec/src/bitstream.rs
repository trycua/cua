// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Annex B / length-prefixed NAL helpers (ITU-T H.264 Annex B, H.265 Annex B).

use std::borrow::Cow;

use crate::types::VideoCodec;

/// Iterates the NAL unit payloads (without start codes) of an Annex B
/// byte stream.
pub fn annex_b_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &s) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(data.len(), |next| next - 3);
        // Trailing zero belongs to a following 4-byte start code.
        while end > s && data[end - 1] == 0 && n + 1 < starts.len() {
            end -= 1;
        }
        if end > s {
            out.push(&data[s..end]);
        }
    }
    out
}

/// NAL unit type of a NAL payload for `codec`.
pub fn nal_type(codec: VideoCodec, nal: &[u8]) -> Option<u8> {
    let first = *nal.first()?;
    match codec {
        VideoCodec::H264 => Some(first & 0x1f),
        VideoCodec::Hevc => Some((first >> 1) & 0x3f),
        VideoCodec::Av1 => None,
    }
}

/// True if the Annex B access unit contains an IDR/IRAP picture.
pub fn is_keyframe(codec: VideoCodec, data: &[u8]) -> bool {
    annex_b_nals(data)
        .iter()
        .any(|nal| match (codec, nal_type(codec, nal)) {
            (VideoCodec::H264, Some(5)) => true,
            (VideoCodec::Hevc, Some(t)) => (16..=23).contains(&t),
            _ => false,
        })
}

/// True if the access unit carries in-band parameter sets (SPS + PPS, and
/// VPS for HEVC).
pub fn has_parameter_sets(codec: VideoCodec, data: &[u8]) -> bool {
    let types: Vec<u8> = annex_b_nals(data)
        .iter()
        .filter_map(|n| nal_type(codec, n))
        .collect();
    match codec {
        VideoCodec::H264 => types.contains(&7) && types.contains(&8),
        VideoCodec::Hevc => types.contains(&32) && types.contains(&33) && types.contains(&34),
        VideoCodec::Av1 => false,
    }
}

/// Converts a length-prefixed (AVCC/HVCC) sample to Annex B. Returns the
/// bytes and whether an IDR was present.
///
/// Adapted from `cua-spacesd-desktop/src/macos_h264.rs` (same repository,
/// Apache-2.0), extended for HEVC.
pub fn length_prefixed_to_annex_b(
    codec: VideoCodec,
    sample: &[u8],
    nal_length_size: usize,
) -> Option<(Vec<u8>, bool)> {
    if !(1..=4).contains(&nal_length_size) {
        return None;
    }
    let mut offset = 0usize;
    let mut output = Vec::with_capacity(sample.len() + 16);
    let mut keyframe = false;
    while offset < sample.len() {
        let header_end = offset.checked_add(nal_length_size)?;
        let header = sample.get(offset..header_end)?;
        let length = header
            .iter()
            .fold(0usize, |v, b| (v << 8) | usize::from(*b));
        offset = header_end;
        let end = offset.checked_add(length)?;
        let nal = sample.get(offset..end)?;
        if nal.is_empty() {
            return None;
        }
        keyframe |= match (codec, nal_type(codec, nal)) {
            (VideoCodec::H264, Some(5)) => true,
            (VideoCodec::Hevc, Some(t)) => (16..=23).contains(&t),
            _ => false,
        };
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
        offset = end;
    }
    (!output.is_empty()).then_some((output, keyframe))
}

/// Converts Annex B NAL units to 4-byte length-prefixed form, skipping NAL
/// types for which `skip` returns true (for example parameter sets that go
/// into a format description instead).
pub fn annex_b_to_length_prefixed(data: &[u8], mut skip: impl FnMut(&[u8]) -> bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for nal in annex_b_nals(data) {
        if skip(nal) {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Returns the H.264 NAL unit `nal` (header byte included, no start code)
/// with a VUI that declares no frame reordering, when it is an SPS without a
/// VUI. Anything else comes back borrowed and unchanged: other NAL types, an
/// SPS that already has a VUI, one with scaling matrices (not parsed here),
/// and a truncated SPS.
///
/// VideoToolbox writes no VUI even in real-time mode with frame reordering
/// off. Without VUI `bitstream_restriction`, H.264 E.2.1 infers
/// `max_num_reorder_frames` as the level's MaxDpbFrames, and decoders that
/// follow it, such as Chrome's hardware decoder on Windows, hold that many
/// frames (4 at 1080p level 4.0) before output. Call this only for encoders
/// that never reorder, which the added VUI states. Decoded pictures do not
/// change.
pub fn h264_low_latency_sps(nal: &[u8]) -> Cow<'_, [u8]> {
    match nal.split_first() {
        Some((&header, payload)) if header & 0x1f == 7 => {
            match no_reordering_sps(&unescape_rbsp(payload)) {
                Some(rbsp) => {
                    let mut out = vec![header];
                    escape_rbsp(&rbsp, &mut out);
                    Cow::Owned(out)
                }
                None => Cow::Borrowed(nal),
            }
        }
        _ => Cow::Borrowed(nal),
    }
}

/// H.264 profiles whose SPS carries chroma format, bit depths and scaling
/// matrices (7.3.2.1.1).
const H264_HIGH_PROFILES: [u32; 13] =
    [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// The SPS RBSP with a VUI that holds only `bitstream_restriction`: no
/// reordering and a decoded picture buffer of `max_num_ref_frames`.
fn no_reordering_sps(rbsp: &[u8]) -> Option<Vec<u8>> {
    let mut read = BitReader { data: rbsp, at: 0 };
    let profile = read.bits(8)?;
    read.bits(16)?; // constraint flags, level_idc
    read.ue()?; // seq_parameter_set_id
    if H264_HIGH_PROFILES.contains(&profile) {
        if read.ue()? == 3 {
            read.bits(1)?; // separate_colour_plane_flag
        }
        read.ue()?; // bit_depth_luma_minus8
        read.ue()?; // bit_depth_chroma_minus8
        read.bits(1)?; // qpprime_y_zero_transform_bypass_flag
        if read.bits(1)? == 1 {
            return None; // seq_scaling_matrix_present_flag
        }
    }
    read.ue()?; // log2_max_frame_num_minus4
    match read.ue()? {
        0 => {
            read.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            read.bits(1)?; // delta_pic_order_always_zero_flag
            read.ue()?; // offset_for_non_ref_pic (se(v) has ue(v)'s length)
            read.ue()?; // offset_for_top_to_bottom_field
            for _ in 0..read.ue()? {
                read.ue()?; // offset_for_ref_frame[i]
            }
        }
        _ => {}
    }
    let max_num_ref_frames = read.ue()?;
    read.bits(1)?; // gaps_in_frame_num_value_allowed_flag
    read.ue()?; // pic_width_in_mbs_minus1
    read.ue()?; // pic_height_in_map_units_minus1
    if read.bits(1)? == 0 {
        read.bits(1)?; // mb_adaptive_frame_field_flag
    }
    read.bits(1)?; // direct_8x8_inference_flag
    if read.bits(1)? == 1 {
        for _ in 0..4 {
            read.ue()?; // frame_crop_*_offset
        }
    }
    let vui_at = read.at;
    if read.bits(1)? == 1 {
        return None; // vui_parameters_present_flag
    }

    let mut write = BitWriter::default();
    for at in 0..vui_at {
        write.bit((rbsp[at / 8] >> (7 - at % 8)) & 1 == 1);
    }
    write.bit(true); // vui_parameters_present_flag
    write.bits(0, 8); // 8 absent parts: aspect ratio through pic_struct
    write.bit(true); // bitstream_restriction_flag
    write.bit(true); // motion_vectors_over_pic_boundaries_flag
    write.ue(2); // max_bytes_per_pic_denom (the inferred default)
    write.ue(1); // max_bits_per_mb_denom (the inferred default)
    write.ue(16); // log2_max_mv_length_horizontal (the inferred default)
    write.ue(16); // log2_max_mv_length_vertical (the inferred default)
    write.ue(0); // max_num_reorder_frames
    write.ue(max_num_ref_frames.max(1)); // max_dec_frame_buffering
    write.bit(true); // rbsp_stop_one_bit; zeros pad to the byte boundary
    Some(write.bytes)
}

/// NAL payload to RBSP: drops each emulation-prevention 0x03 after two zeros.
fn unescape_rbsp(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// RBSP to NAL payload: inserts 0x03 after two zeros before any byte <= 3.
fn escape_rbsp(rbsp: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    at: usize,
}

impl BitReader<'_> {
    fn bits(&mut self, count: u32) -> Option<u32> {
        (0..count).try_fold(0u32, |value, _| {
            let byte = self.data.get(self.at / 8)?;
            let bit = (byte >> (7 - self.at % 8)) & 1;
            self.at += 1;
            Some(value << 1 | u32::from(bit))
        })
    }

    /// Exp-Golomb ue(v); `None` past the end or beyond 32 bits.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    len: usize,
}

impl BitWriter {
    fn bit(&mut self, set: bool) {
        if self.len.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if set {
            *self.bytes.last_mut().expect("pushed above") |= 0x80 >> (self.len % 8);
        }
        self.len += 1;
    }

    fn bits(&mut self, value: u32, count: u32) {
        for shift in (0..count).rev() {
            self.bit((value >> shift) & 1 == 1);
        }
    }

    fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        self.bits(0, length - 1);
        for shift in (0..length).rev() {
            self.bit((code >> shift) & 1 == 1);
        }
    }
}

/// AV1: true if a low-overhead OBU stream contains a sequence header OBU
/// (every key frame we emit starts with one).
pub fn av1_has_sequence_header(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i < data.len() {
        let header = data[i];
        let obu_type = (header >> 3) & 0x0f;
        let has_ext = header & 0x04 != 0;
        let has_size = header & 0x02 != 0;
        if obu_type == 1 {
            return true;
        }
        i += 1 + usize::from(has_ext);
        if !has_size {
            return false;
        }
        // LEB128 size.
        let mut size = 0usize;
        let mut shift = 0;
        loop {
            let Some(&b) = data.get(i) else { return false };
            i += 1;
            size |= usize::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 56 {
                return false;
            }
        }
        i += size;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_prefixed_conversion_marks_idr_and_rejects_truncation() {
        let input = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 2, 0x41, 0xbb];
        let (output, keyframe) = length_prefixed_to_annex_b(VideoCodec::H264, &input, 4).unwrap();
        assert!(keyframe);
        assert_eq!(output, [0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41, 0xbb]);
        assert!(length_prefixed_to_annex_b(VideoCodec::H264, &input[..5], 4).is_none());
        let back = annex_b_to_length_prefixed(&output, |_| false);
        assert_eq!(back, input);
    }

    #[test]
    fn annex_b_split_handles_3_and_4_byte_start_codes() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9,
        ];
        let nals = annex_b_nals(&data);
        assert_eq!(
            nals,
            vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9][..]]
        );
        assert!(is_keyframe(VideoCodec::H264, &data));
        assert!(has_parameter_sets(VideoCodec::H264, &data));
    }

    /// cua-spacesd 0.7.0's VideoToolbox SPS, 1920x1080 Constrained Baseline
    /// level 4.0, recorded from a Mac mini on 2026-11-24 (no VUI).
    const VIDEOTOOLBOX_SPS: [u8; 11] = [
        0x27, 0x42, 0xc0, 0x28, 0xab, 0x40, 0x3c, 0x01, 0x13, 0xf2, 0xa0,
    ];

    #[test]
    fn videotoolbox_sps_gains_a_vui_without_reordering() {
        // ffmpeg's trace_headers reads this as the original SPS plus
        // bitstream_restriction_flag = 1, max_num_reorder_frames = 0 and
        // max_dec_frame_buffering = 1, and the stream decodes bit-identically.
        let patched = h264_low_latency_sps(&VIDEOTOOLBOX_SPS);
        assert_eq!(
            patched.as_ref(),
            [
                0x27, 0x42, 0xc0, 0x28, 0xab, 0x40, 0x3c, 0x01, 0x13, 0xf2, 0xc0, 0x36, 0x82, 0x21,
                0x1a, 0x80,
            ]
        );
        // An SPS that already has a VUI is left alone.
        assert!(matches!(h264_low_latency_sps(&patched), Cow::Borrowed(_)));
    }

    #[test]
    fn low_latency_sps_leaves_other_input_alone() {
        let pps = [0x28, 0xce, 0x3c, 0x80];
        assert!(matches!(h264_low_latency_sps(&pps), Cow::Borrowed(_)));
        assert!(matches!(
            h264_low_latency_sps(&VIDEOTOOLBOX_SPS[..4]),
            Cow::Borrowed(_)
        ));
        assert!(matches!(h264_low_latency_sps(&[]), Cow::Borrowed(_)));
    }

    #[test]
    fn hevc_irap_detection() {
        // IDR_W_RADL = 19 -> header byte 19 << 1 = 0x26.
        let data = [0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x26, 1, 0xaa];
        assert!(is_keyframe(VideoCodec::Hevc, &data));
        let data = [0, 0, 0, 1, 0x02, 1, 0xaa];
        assert!(!is_keyframe(VideoCodec::Hevc, &data));
    }

    #[test]
    fn av1_sequence_header_detection() {
        // temporal delimiter (type 2, size 0), sequence header (type 1, size 1).
        let data = [0x12, 0x00, 0x0a, 0x01, 0x00];
        assert!(av1_has_sequence_header(&data));
        assert!(!av1_has_sequence_header(&[0x12, 0x00]));
    }
}
