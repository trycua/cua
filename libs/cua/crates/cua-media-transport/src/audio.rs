// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Binary audio packets of rcdp wire v2 (MEDIA.md §12.2).
//!
//! One audio packet is one WebSocket binary message or one QUIC datagram: a
//! fixed 24-byte big-endian header followed by the codec payload. The first
//! byte (`R` of the `RAU2` magic, 0x52) tells it apart from a length-prefixed
//! video/control packet, whose first byte is always 0.

use std::fmt;

pub const AUDIO_PACKET_MAGIC: [u8; 4] = *b"RAU2";
pub const AUDIO_PACKET_VERSION: u8 = 2;
pub const AUDIO_PACKET_HEADER_BYTES: usize = 24;
/// Flag bit 0: the previous packet of this track was dropped by the sender,
/// or the track restarted.
pub const AUDIO_FLAG_DISCONTINUITY: u8 = 0b01;
/// Flag bit 1: a DTX/comfort-noise frame after silence.
pub const AUDIO_FLAG_DTX: u8 = 0b10;
const AUDIO_RESERVED_FLAGS: u8 = !(AUDIO_FLAG_DISCONTINUITY | AUDIO_FLAG_DTX);

/// Parsed audio packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacketHeader {
    pub discontinuity: bool,
    pub dtx: bool,
    pub track_id: u16,
    /// Per track and direction; starts anywhere and wraps modulo 2^32.
    pub sequence: u32,
    /// Media-clock microseconds of the first sample.
    pub pts_us: u64,
    /// Samples per channel in this packet.
    pub frame_samples: u16,
    pub config_epoch: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPacketError {
    TooShort(usize),
    InvalidMagic,
    UnsupportedVersion(u8),
    ReservedFlags(u8),
    ReservedByte(u8),
    ZeroTrack,
}

impl fmt::Display for AudioPacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(len) => write!(formatter, "audio packet is too short: {len} bytes"),
            Self::InvalidMagic => formatter.write_str("invalid audio packet magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported audio packet version {version}")
            }
            Self::ReservedFlags(flags) => write!(formatter, "reserved audio flags set: {flags:#x}"),
            Self::ReservedByte(byte) => write!(formatter, "reserved audio byte is {byte}"),
            Self::ZeroTrack => formatter.write_str("audio track id 0 is invalid"),
        }
    }
}

impl std::error::Error for AudioPacketError {}

/// True when a binary media message is an audio packet rather than a
/// length-prefixed packet.
pub fn is_audio_packet(message: &[u8]) -> bool {
    message.len() >= 4 && message[0..4] == AUDIO_PACKET_MAGIC
}

/// Serialize one audio packet (header + payload).
pub fn encode_audio_packet(header: &AudioPacketHeader, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(AUDIO_PACKET_HEADER_BYTES + payload.len());
    packet.extend_from_slice(&AUDIO_PACKET_MAGIC);
    packet.push(AUDIO_PACKET_VERSION);
    let mut flags = 0u8;
    if header.discontinuity {
        flags |= AUDIO_FLAG_DISCONTINUITY;
    }
    if header.dtx {
        flags |= AUDIO_FLAG_DTX;
    }
    packet.push(flags);
    packet.extend_from_slice(&header.track_id.to_be_bytes());
    packet.extend_from_slice(&header.sequence.to_be_bytes());
    packet.extend_from_slice(&header.pts_us.to_be_bytes());
    packet.extend_from_slice(&header.frame_samples.to_be_bytes());
    packet.push(header.config_epoch);
    packet.push(0);
    packet.extend_from_slice(payload);
    packet
}

/// Parse an audio packet into its header and payload slice.
pub fn decode_audio_packet(packet: &[u8]) -> Result<(AudioPacketHeader, &[u8]), AudioPacketError> {
    if packet.len() < AUDIO_PACKET_HEADER_BYTES {
        return Err(AudioPacketError::TooShort(packet.len()));
    }
    if packet[0..4] != AUDIO_PACKET_MAGIC {
        return Err(AudioPacketError::InvalidMagic);
    }
    if packet[4] != AUDIO_PACKET_VERSION {
        return Err(AudioPacketError::UnsupportedVersion(packet[4]));
    }
    let flags = packet[5];
    if flags & AUDIO_RESERVED_FLAGS != 0 {
        return Err(AudioPacketError::ReservedFlags(flags));
    }
    if packet[23] != 0 {
        return Err(AudioPacketError::ReservedByte(packet[23]));
    }
    let track_id = u16::from_be_bytes([packet[6], packet[7]]);
    if track_id == 0 {
        return Err(AudioPacketError::ZeroTrack);
    }
    let header = AudioPacketHeader {
        discontinuity: flags & AUDIO_FLAG_DISCONTINUITY != 0,
        dtx: flags & AUDIO_FLAG_DTX != 0,
        track_id,
        sequence: u32::from_be_bytes(packet[8..12].try_into().expect("four bytes")),
        pts_us: u64::from_be_bytes(packet[12..20].try_into().expect("eight bytes")),
        frame_samples: u16::from_be_bytes([packet[20], packet[21]]),
        config_epoch: packet[22],
    };
    Ok((header, &packet[AUDIO_PACKET_HEADER_BYTES..]))
}

/// RFC 1982 serial-number comparison for 32-bit audio sequences: true when
/// `a` comes after `b`.
pub fn sequence_after(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000_0000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> AudioPacketHeader {
        AudioPacketHeader {
            discontinuity: true,
            dtx: false,
            track_id: 7,
            sequence: u32::MAX,
            pts_us: 1_234_567,
            frame_samples: 960,
            config_epoch: 3,
        }
    }

    #[test]
    fn round_trips_and_has_the_documented_layout() {
        let packet = encode_audio_packet(&header(), &[9, 8, 7]);
        assert_eq!(&packet[0..4], b"RAU2");
        assert_eq!(packet[4], 2);
        assert_eq!(packet[5], 1);
        assert_eq!(&packet[6..8], &[0, 7]);
        assert_eq!(&packet[20..22], &960u16.to_be_bytes());
        assert_eq!(packet[22], 3);
        assert_eq!(packet[23], 0);
        assert_eq!(packet.len(), 27);
        assert!(is_audio_packet(&packet));
        let (decoded, payload) = decode_audio_packet(&packet).unwrap();
        assert_eq!(decoded, header());
        assert_eq!(payload, &[9, 8, 7]);
    }

    #[test]
    fn rejects_bad_packets_without_panicking() {
        let mut packet = encode_audio_packet(&header(), &[]);
        assert!(decode_audio_packet(&packet[..10]).is_err());
        packet[5] = 0b100;
        assert_eq!(
            decode_audio_packet(&packet).unwrap_err(),
            AudioPacketError::ReservedFlags(4)
        );
        let mut zero = encode_audio_packet(
            &AudioPacketHeader {
                track_id: 0,
                ..header()
            },
            &[],
        );
        assert_eq!(
            decode_audio_packet(&zero).unwrap_err(),
            AudioPacketError::ZeroTrack
        );
        zero[0] = 0;
        assert!(!is_audio_packet(&zero));
    }

    #[test]
    fn serial_comparison_handles_wraparound() {
        assert!(sequence_after(0, u32::MAX));
        assert!(sequence_after(5, 3));
        assert!(!sequence_after(3, 5));
        assert!(!sequence_after(3, 3));
    }
}
