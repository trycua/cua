// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The presence datagram channel (`cua-presence/1`): cursor records as
//! unreliable, latest-wins QUIC datagrams. Wire spec:
//! `libs/cua/proto/PRESENCE.md` section 6.
//!
//! Shared by cua-spacesd (server) and the SDK (client) so both ends encode
//! the same bytes.

use serde::{Deserialize, Serialize};

/// QUIC ALPN of the presence channel.
pub const PRESENCE_ALPN: &str = "cua-presence/1";
/// Datagram magic.
pub const MAGIC: &[u8; 4] = b"RPC1";
/// Datagram version byte.
pub const VERSION: u8 = 1;
/// Header length in bytes.
pub const HEADER_LEN: usize = 19;
/// Record length in bytes.
pub const RECORD_LEN: usize = 9;
/// Largest datagram the channel sends.
pub const MAX_DATAGRAM: usize = 1200;
/// Most records that fit in one datagram.
pub const MAX_RECORDS: usize = (MAX_DATAGRAM - HEADER_LEN) / RECORD_LEN;
/// Header flag: the datagram carries the full cursor set.
pub const FLAG_KEYFRAME: u8 = 1;

const STATE_VISIBLE: u8 = 1;
const STATE_PRESSED: u8 = 2;

/// One cursor on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorRecord {
    /// Participant slot (from the `slots` control message).
    pub slot: u8,
    /// Per-participant sequence, wrapping.
    pub seq: u16,
    /// Quantized x (see [`quantize`]).
    pub x: u16,
    /// Quantized y.
    pub y: u16,
    /// `cua.env.v1.CursorShape` value.
    pub shape: u8,
    /// Visible.
    pub visible: bool,
    /// A button is down.
    pub pressed: bool,
    /// `cua.env.v1.CursorShapeSource` value (0..=3).
    pub shape_source: u8,
}

impl CursorRecord {
    /// Normalized position.
    pub fn position(&self) -> (f64, f64) {
        (dequantize(self.x), dequantize(self.y))
    }
}

/// One datagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorDatagram {
    /// Header flags ([`FLAG_KEYFRAME`]).
    pub flags: u8,
    /// Server tick (downlink) or client counter (uplink).
    pub tick: u32,
    /// Server clock in microseconds (downlink), 0 uplink.
    pub server_time_us: u64,
    /// At most [`MAX_RECORDS`].
    pub records: Vec<CursorRecord>,
}

/// Why a datagram was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Shorter than its header or record count says.
    Truncated,
    /// Not `RPC1` version 1.
    BadMagic,
    /// Reserved bits set.
    Reserved,
}

/// `round(v * 65535)` of a normalized coordinate, clamped to `[0, 1]`.
pub fn quantize(v: f64) -> u16 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// Inverse of [`quantize`].
pub fn dequantize(v: u16) -> f64 {
    f64::from(v) / 65535.0
}

/// Whether sequence `a` is newer than `b` (serial-number arithmetic, RFC
/// 1982 style, over u16).
pub fn seq_newer(a: u16, b: u16) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000
}

impl CursorDatagram {
    /// Encode. Records beyond [`MAX_RECORDS`] are dropped; callers split
    /// larger sets with [`CursorDatagram::split`].
    pub fn encode(&self) -> Vec<u8> {
        let records = &self.records[..self.records.len().min(MAX_RECORDS)];
        let mut out = Vec::with_capacity(HEADER_LEN + RECORD_LEN * records.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(self.flags & FLAG_KEYFRAME);
        out.extend_from_slice(&self.tick.to_be_bytes());
        out.extend_from_slice(&self.server_time_us.to_be_bytes());
        out.push(records.len() as u8);
        for r in records {
            out.push(r.slot);
            out.extend_from_slice(&r.seq.to_be_bytes());
            out.extend_from_slice(&r.x.to_be_bytes());
            out.extend_from_slice(&r.y.to_be_bytes());
            out.push(r.shape);
            let mut state = 0u8;
            if r.visible {
                state |= STATE_VISIBLE;
            }
            if r.pressed {
                state |= STATE_PRESSED;
            }
            state |= (r.shape_source & 0b11) << 2;
            out.push(state);
        }
        out
    }

    /// Decode one datagram.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Truncated);
        }
        if &bytes[0..4] != MAGIC || bytes[4] != VERSION {
            return Err(DecodeError::BadMagic);
        }
        let flags = bytes[5];
        if flags & !FLAG_KEYFRAME != 0 {
            return Err(DecodeError::Reserved);
        }
        let tick = u32::from_be_bytes(bytes[6..10].try_into().unwrap());
        let server_time_us = u64::from_be_bytes(bytes[10..18].try_into().unwrap());
        let count = usize::from(bytes[18]);
        if bytes.len() < HEADER_LEN + count * RECORD_LEN {
            return Err(DecodeError::Truncated);
        }
        let mut records = Vec::with_capacity(count);
        for i in 0..count {
            let r = &bytes[HEADER_LEN + i * RECORD_LEN..HEADER_LEN + (i + 1) * RECORD_LEN];
            let state = r[8];
            if state & 0b1111_0000 != 0 {
                return Err(DecodeError::Reserved);
            }
            records.push(CursorRecord {
                slot: r[0],
                seq: u16::from_be_bytes([r[1], r[2]]),
                x: u16::from_be_bytes([r[3], r[4]]),
                y: u16::from_be_bytes([r[5], r[6]]),
                shape: r[7],
                visible: state & STATE_VISIBLE != 0,
                pressed: state & STATE_PRESSED != 0,
                shape_source: (state >> 2) & 0b11,
            });
        }
        Ok(Self {
            flags,
            tick,
            server_time_us,
            records,
        })
    }

    /// Split into datagrams of at most [`MAX_RECORDS`] records each, sharing
    /// the header fields.
    pub fn split(self) -> Vec<CursorDatagram> {
        if self.records.len() <= MAX_RECORDS {
            return vec![self];
        }
        self.records
            .chunks(MAX_RECORDS)
            .map(|chunk| CursorDatagram {
                flags: self.flags,
                tick: self.tick,
                server_time_us: self.server_time_us,
                records: chunk.to_vec(),
            })
            .collect()
    }
}

/// Reliable-stream control messages of the presence channel, one JSON object
/// per line: `{"type": ..., "payload": ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PresenceControl {
    /// Client -> server, first line: bind this connection to a participant.
    PresenceTicket {
        /// `PresenceDatagrams.ticket`.
        ticket: String,
    },
    /// Server -> client: slot table (slot as a decimal string key) and the
    /// recipient's own slot.
    Slots {
        /// Slot -> participant id.
        slots: std::collections::BTreeMap<String, String>,
        /// The recipient's slot.
        you: u8,
    },
    /// Either direction: a cursor's target changed.
    CursorTarget {
        /// Whose.
        slot: u8,
        /// Display id; empty means the primary display.
        #[serde(default)]
        display_id: String,
        /// Window, when the cursor is over a window stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<PresenceWindowRef>,
    },
}

/// A window reference on the presence channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceWindowRef {
    /// Window id.
    pub id: String,
    /// Window epoch.
    #[serde(default)]
    pub epoch: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(slot: u8, seq: u16) -> CursorRecord {
        CursorRecord {
            slot,
            seq,
            x: quantize(0.25),
            y: quantize(0.75),
            shape: 2,
            visible: true,
            pressed: false,
            shape_source: 3,
        }
    }

    #[test]
    fn round_trips_and_is_nine_bytes_a_cursor() {
        let d = CursorDatagram {
            flags: FLAG_KEYFRAME,
            tick: 0xdead_beef,
            server_time_us: 1_234_567_890,
            records: vec![record(1, 7), record(2, 65535)],
        };
        let bytes = d.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 2 * RECORD_LEN);
        assert_eq!(&bytes[..4], b"RPC1");
        assert_eq!(CursorDatagram::decode(&bytes).unwrap(), d);
        let (x, y) = d.records[0].position();
        assert!((x - 0.25).abs() < 1e-4 && (y - 0.75).abs() < 1e-4);
    }

    #[test]
    fn quantization_is_sub_pixel_at_8k() {
        for i in 0..=7680 {
            let v = f64::from(i) / 7680.0;
            let px = dequantize(quantize(v)) * 7680.0;
            assert!((px - f64::from(i)).abs() < 0.2, "{i} -> {px}");
        }
        assert_eq!(quantize(-1.0), 0);
        assert_eq!(quantize(2.0), 65535);
        assert_eq!(quantize(f64::NAN), 0);
    }

    #[test]
    fn sequence_comparison_wraps() {
        assert!(seq_newer(1, 0));
        assert!(seq_newer(0, 65535));
        assert!(!seq_newer(65535, 0));
        assert!(!seq_newer(5, 5));
        assert!(!seq_newer(0, 0x8000));
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(CursorDatagram::decode(b"RPC1"), Err(DecodeError::Truncated));
        let mut bytes = CursorDatagram {
            flags: 0,
            tick: 1,
            server_time_us: 0,
            records: vec![record(1, 1)],
        }
        .encode();
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(CursorDatagram::decode(&bad), Err(DecodeError::BadMagic));
        let mut reserved = bytes.clone();
        reserved[5] = 0x80;
        assert_eq!(
            CursorDatagram::decode(&reserved),
            Err(DecodeError::Reserved)
        );
        bytes.pop();
        assert_eq!(CursorDatagram::decode(&bytes), Err(DecodeError::Truncated));
    }

    #[test]
    fn large_sets_split_under_the_mtu() {
        let d = CursorDatagram {
            flags: FLAG_KEYFRAME,
            tick: 9,
            server_time_us: 1,
            records: (0..=255u8).map(|s| record(s, 1)).collect(),
        };
        let parts = d.split();
        assert_eq!(parts.iter().map(|p| p.records.len()).sum::<usize>(), 256);
        for p in parts {
            assert!(p.encode().len() <= MAX_DATAGRAM);
        }
    }

    #[test]
    fn control_messages_use_the_type_payload_shape() {
        let m = PresenceControl::CursorTarget {
            slot: 3,
            display_id: String::new(),
            window: Some(PresenceWindowRef {
                id: "w1".into(),
                epoch: 2,
            }),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(
            json,
            r#"{"type":"cursor_target","payload":{"slot":3,"display_id":"","window":{"id":"w1","epoch":2}}}"#
        );
        assert_eq!(serde_json::from_str::<PresenceControl>(&json).unwrap(), m);
        let t: PresenceControl =
            serde_json::from_str(r#"{"type":"presence_ticket","payload":{"ticket":"t"}}"#).unwrap();
        assert_eq!(t, PresenceControl::PresenceTicket { ticket: "t".into() });
    }
}
