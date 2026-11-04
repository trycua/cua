// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Annex B / length-prefixed NAL helpers (ITU-T H.264 Annex B, H.265 Annex B).

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
