// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The 24-byte `RAU2` audio packet header (MEDIA.md §12.2), big-endian.

use crate::error::{CodecError, Result};

/// Magic bytes.
pub const MAGIC: [u8; 4] = *b"RAU2";
/// Header length.
pub const HEADER_LEN: usize = 24;
/// Wire version.
pub const VERSION: u8 = 2;
/// Flag: the previous packet on this track was dropped by the sender.
pub const FLAG_DISCONTINUITY: u8 = 1 << 0;
/// Flag: DTX / comfort-noise frame after silence.
pub const FLAG_DTX: u8 = 1 << 1;

/// Parsed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacketHeader {
    /// Flags (bits 0-1 defined).
    pub flags: u8,
    /// Track id (never 0).
    pub track_id: u16,
    /// Per-track sequence number (wraps).
    pub sequence: u32,
    /// Media-clock time of the first sample.
    pub pts_us: u64,
    /// Samples per channel.
    pub frame_samples: u16,
    /// Must match the latest `audio_config`.
    pub config_epoch: u8,
}

impl AudioPacketHeader {
    /// Serialises header + payload into one message.
    pub fn encode(&self, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(self.flags & (FLAG_DISCONTINUITY | FLAG_DTX));
        out.extend_from_slice(&self.track_id.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.pts_us.to_be_bytes());
        out.extend_from_slice(&self.frame_samples.to_be_bytes());
        out.push(self.config_epoch);
        out.push(0);
        out.extend_from_slice(payload);
        out
    }

    /// Parses a message into header and payload.
    pub fn decode(message: &[u8]) -> Result<(Self, &[u8])> {
        if message.len() < HEADER_LEN || message[..4] != MAGIC {
            return Err(CodecError::InvalidArgument(
                "not an RAU2 audio packet".into(),
            ));
        }
        if message[4] != VERSION
            || message[5] & !(FLAG_DISCONTINUITY | FLAG_DTX) != 0
            || message[23] != 0
        {
            return Err(CodecError::InvalidArgument(
                "bad RAU2 version or reserved bits".into(),
            ));
        }
        let track_id = u16::from_be_bytes([message[6], message[7]]);
        if track_id == 0 {
            return Err(CodecError::InvalidArgument("track id 0".into()));
        }
        Ok((
            Self {
                flags: message[5],
                track_id,
                sequence: u32::from_be_bytes(message[8..12].try_into().expect("4 bytes")),
                pts_us: u64::from_be_bytes(message[12..20].try_into().expect("8 bytes")),
                frame_samples: u16::from_be_bytes([message[20], message[21]]),
                config_epoch: message[22],
            },
            &message[HEADER_LEN..],
        ))
    }
}

/// RFC 1982 serial comparison: signed distance from `a` to `b`.
pub fn sequence_distance(a: u32, b: u32) -> i64 {
    i64::from(b.wrapping_sub(a) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_layout() {
        let h = AudioPacketHeader {
            flags: FLAG_DTX,
            track_id: 3,
            sequence: 0xFFFF_FFFF,
            pts_us: 1_234_567,
            frame_samples: 960,
            config_epoch: 7,
        };
        let msg = h.encode(&[9, 9]);
        assert_eq!(msg.len(), HEADER_LEN + 2);
        assert_eq!(msg[0], 0x52);
        assert_eq!(&msg[20..22], &960u16.to_be_bytes());
        let (back, payload) = AudioPacketHeader::decode(&msg).unwrap();
        assert_eq!(back, h);
        assert_eq!(payload, &[9, 9]);
        let mut bad = msg.clone();
        bad[23] = 1;
        assert!(AudioPacketHeader::decode(&bad).is_err());
    }

    #[test]
    fn serial_distance_wraps() {
        assert_eq!(sequence_distance(u32::MAX, 1), 2);
        assert_eq!(sequence_distance(5, 3), -2);
    }
}
