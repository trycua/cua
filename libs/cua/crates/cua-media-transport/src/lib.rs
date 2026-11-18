// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Local RCDP packet framing.
//!
//! Each packet is `header_len:u32 | payload_len:u32 | JSON header | payload`,
//! with network-byte-order lengths. Video, icons, and bounded file-clipboard
//! messages may carry binary payloads; other control packets may not.

use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

use cua_media_protocol::WireHeader;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_HEADER_BYTES: usize = 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const VIDEO_DATAGRAM_HEADER_BYTES: usize = 24;
pub const DEFAULT_VIDEO_REASSEMBLY_DEADLINE: Duration = Duration::from_millis(150);

const VIDEO_DATAGRAM_MAGIC: [u8; 4] = *b"RVD1";
const VIDEO_DATAGRAM_VERSION: u8 = 1;
const VIDEO_DATAGRAM_MAGIC_V2: [u8; 4] = *b"RVD2";
const VIDEO_DATAGRAM_VERSION_V2: u8 = 2;
const VIDEO_DATAGRAM_KEYFRAME: u8 = 1;

pub mod audio;
#[cfg(feature = "quic")]
pub mod quic;

/// Which rcdp wire version a video datagram belongs to. v2 (MEDIA.md §9)
/// changes only the magic (`RVD2`) and the version byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramVersion {
    V1,
    V2,
}

impl DatagramVersion {
    const fn magic(self) -> [u8; 4] {
        match self {
            Self::V1 => VIDEO_DATAGRAM_MAGIC,
            Self::V2 => VIDEO_DATAGRAM_MAGIC_V2,
        }
    }

    const fn byte(self) -> u8 {
        match self {
            Self::V1 => VIDEO_DATAGRAM_VERSION,
            Self::V2 => VIDEO_DATAGRAM_VERSION_V2,
        }
    }
}

fn allows_binary_payload(header: &WireHeader) -> bool {
    matches!(
        header,
        WireHeader::Video(_)
            | WireHeader::AppIcon(_)
            | WireHeader::Client(cua_media_protocol::ClientMessage::SetClipboardFiles { .. })
            | WireHeader::Server(cua_media_protocol::ServerMessage::ClipboardFiles { .. })
    )
}

#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    Json(serde_json::Error),
    HeaderTooLarge(usize),
    PayloadTooLarge(usize),
    UnexpectedPayload,
    InvalidPacketLength { expected: usize, actual: usize },
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "transport I/O: {error}"),
            Self::Json(error) => write!(formatter, "invalid packet header: {error}"),
            Self::HeaderTooLarge(size) => write!(formatter, "header is too large: {size} bytes"),
            Self::PayloadTooLarge(size) => write!(formatter, "payload is too large: {size} bytes"),
            Self::UnexpectedPayload => formatter.write_str("control packet carried a payload"),
            Self::InvalidPacketLength { expected, actual } => write!(
                formatter,
                "invalid packet length: expected {expected} bytes, got {actual}"
            ),
        }
    }
}

impl Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for TransportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatagramError {
    DatagramTooSmall(usize),
    PacketTooLarge(usize),
    TooManyFragments(usize),
    InvalidMagic,
    UnsupportedVersion(u8),
    InvalidFragmentIndex { index: u16, count: u16 },
    InconsistentFrame,
    InvalidReassembledLength { expected: usize, actual: usize },
}

impl fmt::Display for DatagramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatagramTooSmall(size) => {
                write!(formatter, "video datagram is too small: {size} bytes")
            }
            Self::PacketTooLarge(size) => {
                write!(formatter, "video packet is too large: {size} bytes")
            }
            Self::TooManyFragments(count) => {
                write!(formatter, "video packet needs too many fragments: {count}")
            }
            Self::InvalidMagic => formatter.write_str("invalid video datagram magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported video datagram version {version}")
            }
            Self::InvalidFragmentIndex { index, count } => {
                write!(formatter, "invalid video fragment index {index} of {count}")
            }
            Self::InconsistentFrame => {
                formatter.write_str("video fragments disagree about frame metadata")
            }
            Self::InvalidReassembledLength { expected, actual } => write!(
                formatter,
                "invalid reassembled video length: expected {expected}, got {actual}"
            ),
        }
    }
}

impl Error for DatagramError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoDatagramFragment<'a> {
    pub version: DatagramVersion,
    pub packet_id: u64,
    pub keyframe: bool,
    pub fragment_index: u16,
    pub fragment_count: u16,
    pub packet_len: u32,
    pub payload: &'a [u8],
}

pub fn fragment_video_packet(
    packet_id: u64,
    keyframe: bool,
    packet: &[u8],
    max_datagram_size: usize,
) -> Result<Vec<Vec<u8>>, DatagramError> {
    fragment_video_packet_versioned(
        DatagramVersion::V1,
        packet_id,
        keyframe,
        packet,
        max_datagram_size,
    )
}

/// Fragment one length-prefixed video packet into `RVD2` datagrams.
pub fn fragment_video_packet_v2(
    packet_id: u64,
    keyframe: bool,
    packet: &[u8],
    max_datagram_size: usize,
) -> Result<Vec<Vec<u8>>, DatagramError> {
    fragment_video_packet_versioned(
        DatagramVersion::V2,
        packet_id,
        keyframe,
        packet,
        max_datagram_size,
    )
}

pub fn fragment_video_packet_versioned(
    version: DatagramVersion,
    packet_id: u64,
    keyframe: bool,
    packet: &[u8],
    max_datagram_size: usize,
) -> Result<Vec<Vec<u8>>, DatagramError> {
    if max_datagram_size <= VIDEO_DATAGRAM_HEADER_BYTES {
        return Err(DatagramError::DatagramTooSmall(max_datagram_size));
    }
    let packet_len =
        u32::try_from(packet.len()).map_err(|_| DatagramError::PacketTooLarge(packet.len()))?;
    if packet.len() > MAX_PAYLOAD_BYTES + MAX_HEADER_BYTES + 8 {
        return Err(DatagramError::PacketTooLarge(packet.len()));
    }
    let chunk_size = max_datagram_size - VIDEO_DATAGRAM_HEADER_BYTES;
    let fragment_count = packet.len().div_ceil(chunk_size).max(1);
    let fragment_count_u16 = u16::try_from(fragment_count)
        .map_err(|_| DatagramError::TooManyFragments(fragment_count))?;
    let flags = u8::from(keyframe) * VIDEO_DATAGRAM_KEYFRAME;
    let mut fragments = Vec::with_capacity(fragment_count);
    for index in 0..fragment_count {
        let start = index.saturating_mul(chunk_size);
        let end = start.saturating_add(chunk_size).min(packet.len());
        let payload = packet.get(start..end).unwrap_or_default();
        let mut datagram = Vec::with_capacity(VIDEO_DATAGRAM_HEADER_BYTES + payload.len());
        datagram.extend_from_slice(&version.magic());
        datagram.push(version.byte());
        datagram.push(flags);
        datagram.extend_from_slice(&(index as u16).to_be_bytes());
        datagram.extend_from_slice(&fragment_count_u16.to_be_bytes());
        datagram.extend_from_slice(&0_u16.to_be_bytes());
        datagram.extend_from_slice(&packet_id.to_be_bytes());
        datagram.extend_from_slice(&packet_len.to_be_bytes());
        datagram.extend_from_slice(payload);
        fragments.push(datagram);
    }
    Ok(fragments)
}

pub fn decode_video_datagram(datagram: &[u8]) -> Result<VideoDatagramFragment<'_>, DatagramError> {
    if datagram.len() < VIDEO_DATAGRAM_HEADER_BYTES {
        return Err(DatagramError::DatagramTooSmall(datagram.len()));
    }
    let version = if datagram[0..4] == VIDEO_DATAGRAM_MAGIC {
        DatagramVersion::V1
    } else if datagram[0..4] == VIDEO_DATAGRAM_MAGIC_V2 {
        DatagramVersion::V2
    } else {
        return Err(DatagramError::InvalidMagic);
    };
    if datagram[4] != version.byte() {
        return Err(DatagramError::UnsupportedVersion(datagram[4]));
    }
    let fragment_index = u16::from_be_bytes([datagram[6], datagram[7]]);
    let fragment_count = u16::from_be_bytes([datagram[8], datagram[9]]);
    if fragment_count == 0 || fragment_index >= fragment_count {
        return Err(DatagramError::InvalidFragmentIndex {
            index: fragment_index,
            count: fragment_count,
        });
    }
    let packet_id = u64::from_be_bytes(datagram[12..20].try_into().expect("eight-byte packet ID"));
    let packet_len = u32::from_be_bytes(datagram[20..24].try_into().expect("four-byte length"));
    if packet_len as usize > MAX_PAYLOAD_BYTES + MAX_HEADER_BYTES + 8 {
        return Err(DatagramError::PacketTooLarge(packet_len as usize));
    }
    Ok(VideoDatagramFragment {
        version,
        packet_id,
        keyframe: datagram[5] & VIDEO_DATAGRAM_KEYFRAME != 0,
        fragment_index,
        fragment_count,
        packet_len,
        payload: &datagram[VIDEO_DATAGRAM_HEADER_BYTES..],
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReassemblyUpdate {
    pub packet: Option<Vec<u8>>,
    pub dropped_incomplete: u64,
    pub lost_keyframe: bool,
}

#[derive(Debug)]
struct PendingVideoPacket {
    packet_id: u64,
    keyframe: bool,
    packet_len: u32,
    started_at: Instant,
    remaining: usize,
    fragments: Vec<Option<Vec<u8>>>,
}

#[derive(Debug)]
pub struct VideoDatagramReassembler {
    deadline: Duration,
    latest_packet_id: Option<u64>,
    pending: Option<PendingVideoPacket>,
}

impl Default for VideoDatagramReassembler {
    fn default() -> Self {
        Self::new(DEFAULT_VIDEO_REASSEMBLY_DEADLINE)
    }
}

impl VideoDatagramReassembler {
    pub const fn new(deadline: Duration) -> Self {
        Self {
            deadline,
            latest_packet_id: None,
            pending: None,
        }
    }

    pub fn push(
        &mut self,
        datagram: &[u8],
        now: Instant,
    ) -> Result<ReassemblyUpdate, DatagramError> {
        let fragment = decode_video_datagram(datagram)?;
        let mut update = ReassemblyUpdate::default();
        if self.pending.as_ref().is_some_and(|pending| {
            now.saturating_duration_since(pending.started_at) > self.deadline
        }) {
            let expired = self.pending.take().expect("pending packet was checked");
            update.dropped_incomplete = 1;
            update.lost_keyframe = expired.keyframe;
        }

        if self
            .latest_packet_id
            .is_some_and(|latest| fragment.packet_id < latest)
        {
            return Ok(update);
        }
        if self.latest_packet_id != Some(fragment.packet_id) {
            if let Some(replaced) = self.pending.take() {
                update.dropped_incomplete = update.dropped_incomplete.saturating_add(1);
                update.lost_keyframe |= replaced.keyframe;
            }
            self.latest_packet_id = Some(fragment.packet_id);
            self.pending = Some(PendingVideoPacket {
                packet_id: fragment.packet_id,
                keyframe: fragment.keyframe,
                packet_len: fragment.packet_len,
                started_at: now,
                remaining: usize::from(fragment.fragment_count),
                fragments: vec![None; usize::from(fragment.fragment_count)],
            });
        }
        let Some(pending) = self.pending.as_mut() else {
            return Ok(update);
        };
        if pending.packet_id != fragment.packet_id
            || pending.keyframe != fragment.keyframe
            || pending.packet_len != fragment.packet_len
            || pending.fragments.len() != usize::from(fragment.fragment_count)
        {
            return Err(DatagramError::InconsistentFrame);
        }
        let slot = &mut pending.fragments[usize::from(fragment.fragment_index)];
        if slot.is_none() {
            *slot = Some(fragment.payload.to_vec());
            pending.remaining = pending.remaining.saturating_sub(1);
        }
        if pending.remaining != 0 {
            return Ok(update);
        }
        let pending = self.pending.take().expect("completed packet exists");
        let expected = pending.packet_len as usize;
        let mut packet = Vec::with_capacity(expected);
        for fragment in pending.fragments {
            packet.extend(fragment.expect("all completed fragments"));
        }
        if packet.len() != expected {
            return Err(DatagramError::InvalidReassembledLength {
                expected,
                actual: packet.len(),
            });
        }
        update.packet = Some(packet);
        Ok(update)
    }
}

pub async fn read_packet<R>(reader: &mut R) -> Result<Option<(WireHeader, Vec<u8>)>, TransportError>
where
    R: AsyncRead + Unpin,
{
    let header_len = match reader.read_u32().await {
        Ok(length) => length as usize,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let payload_len = reader.read_u32().await? as usize;
    if header_len > MAX_HEADER_BYTES {
        return Err(TransportError::HeaderTooLarge(header_len));
    }
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(TransportError::PayloadTooLarge(payload_len));
    }
    let mut header = vec![0; header_len];
    reader.read_exact(&mut header).await?;
    let mut payload = vec![0; payload_len];
    reader.read_exact(&mut payload).await?;
    let header = serde_json::from_slice(&header)?;
    if !allows_binary_payload(&header) && !payload.is_empty() {
        return Err(TransportError::UnexpectedPayload);
    }
    Ok(Some((header, payload)))
}

pub async fn write_packet<W>(
    writer: &mut W,
    header: &WireHeader,
    payload: &[u8],
) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
{
    let packet = encode_packet(header, payload)?;
    writer.write_all(&packet).await?;
    writer.flush().await?;
    Ok(())
}

/// Encodes one complete packet for message-oriented transports such as a
/// WebSocket binary message.
pub fn encode_packet(header: &WireHeader, payload: &[u8]) -> Result<Vec<u8>, TransportError> {
    if !allows_binary_payload(header) && !payload.is_empty() {
        return Err(TransportError::UnexpectedPayload);
    }
    let header = serde_json::to_vec(header)?;
    if header.len() > MAX_HEADER_BYTES {
        return Err(TransportError::HeaderTooLarge(header.len()));
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(TransportError::PayloadTooLarge(payload.len()));
    }
    let mut packet = Vec::with_capacity(8 + header.len() + payload.len());
    packet.extend_from_slice(&(header.len() as u32).to_be_bytes());
    packet.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    packet.extend_from_slice(&header);
    packet.extend_from_slice(payload);
    Ok(packet)
}

/// Decodes exactly one packet from a message-oriented transport.
pub fn decode_packet(packet: &[u8]) -> Result<(WireHeader, Vec<u8>), TransportError> {
    if packet.len() < 8 {
        return Err(TransportError::InvalidPacketLength {
            expected: 8,
            actual: packet.len(),
        });
    }
    let header_len =
        u32::from_be_bytes(packet[0..4].try_into().expect("four-byte header")) as usize;
    let payload_len =
        u32::from_be_bytes(packet[4..8].try_into().expect("four-byte payload")) as usize;
    if header_len > MAX_HEADER_BYTES {
        return Err(TransportError::HeaderTooLarge(header_len));
    }
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(TransportError::PayloadTooLarge(payload_len));
    }
    let expected = 8usize
        .checked_add(header_len)
        .and_then(|size| size.checked_add(payload_len))
        .ok_or(TransportError::PayloadTooLarge(payload_len))?;
    if packet.len() != expected {
        return Err(TransportError::InvalidPacketLength {
            expected,
            actual: packet.len(),
        });
    }
    let header = serde_json::from_slice(&packet[8..8 + header_len])?;
    let payload = packet[8 + header_len..].to_vec();
    if !allows_binary_payload(&header) && !payload.is_empty() {
        return Err(TransportError::UnexpectedPayload);
    }
    Ok((header, payload))
}

#[cfg(test)]
mod tests {
    use cua_media_protocol::{
        AppIconDescriptor, ClientMessage, ClipboardFile, Hello, ServerMessage, TargetEpoch,
        TargetHandle,
    };

    use super::*;

    #[tokio::test]
    async fn control_packet_round_trips_without_payload() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let header = WireHeader::Client(ClientMessage::Hello(Hello::default()));
        let expected = header.clone();
        let send = tokio::spawn(async move { write_packet(&mut writer, &header, &[]).await });
        let (received, payload) = read_packet(&mut reader).await.unwrap().unwrap();
        send.await.unwrap().unwrap();
        assert_eq!(received, expected);
        assert!(payload.is_empty());
    }

    #[tokio::test]
    async fn oversized_header_is_rejected_before_allocation() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        let send = tokio::spawn(async move {
            writer
                .write_u32((MAX_HEADER_BYTES + 1) as u32)
                .await
                .unwrap();
            writer.write_u32(0).await.unwrap();
        });
        assert!(matches!(
            read_packet(&mut reader).await,
            Err(TransportError::HeaderTooLarge(_))
        ));
        send.await.unwrap();
    }

    #[test]
    fn message_packet_round_trips() {
        let header = WireHeader::Client(ClientMessage::Hello(Hello::default()));
        let packet = encode_packet(&header, &[]).unwrap();
        let (decoded, payload) = decode_packet(&packet).unwrap();
        assert_eq!(decoded, header);
        assert!(payload.is_empty());
    }

    #[test]
    fn application_icon_round_trips_as_a_binary_asset() {
        let header = WireHeader::AppIcon(AppIconDescriptor {
            window: TargetHandle("target-opaque".into()),
            target_epoch: TargetEpoch(3),
            media_type: "application/x-apple-icns".into(),
            byte_len: 4,
        });
        let packet = encode_packet(&header, &[1, 2, 3, 4]).unwrap();
        let (decoded, payload) = decode_packet(&packet).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(payload, [1, 2, 3, 4]);
    }

    #[test]
    fn clipboard_files_round_trip_as_binary_assets_in_both_directions() {
        let file = ClipboardFile {
            name: "note.txt".into(),
            offset: 0,
            byte_len: 4,
            sha256: "transport-does-not-inspect-checksums".into(),
        };
        let headers = [
            WireHeader::Client(ClientMessage::SetClipboardFiles {
                files: vec![file.clone()],
                byte_len: 4,
            }),
            WireHeader::Server(ServerMessage::ClipboardFiles {
                generation: 7,
                files: Some(vec![file]),
                byte_len: 4,
            }),
        ];
        for header in headers {
            let packet = encode_packet(&header, &[1, 2, 3, 4]).unwrap();
            let (decoded, payload) = decode_packet(&packet).unwrap();
            assert_eq!(decoded, header);
            assert_eq!(payload, [1, 2, 3, 4]);
        }
    }

    #[test]
    fn ordinary_control_packets_reject_binary_payloads() {
        let header = WireHeader::Client(ClientMessage::Hello(Hello::default()));
        assert!(matches!(
            encode_packet(&header, &[1]),
            Err(TransportError::UnexpectedPayload)
        ));
    }

    #[test]
    fn video_datagrams_reassemble_out_of_order() {
        let packet = (0_u8..=250).cycle().take(4_000).collect::<Vec<_>>();
        let mut fragments = fragment_video_packet(7, false, &packet, 1_200).unwrap();
        fragments.reverse();
        let now = Instant::now();
        let mut reassembler = VideoDatagramReassembler::default();
        let mut completed = None;
        for fragment in fragments {
            let update = reassembler.push(&fragment, now).unwrap();
            assert_eq!(update.dropped_incomplete, 0);
            completed = completed.or(update.packet);
        }
        assert_eq!(completed.as_deref(), Some(packet.as_slice()));
    }

    #[test]
    fn newer_video_datagram_expires_an_obsolete_partial_frame() {
        let old = fragment_video_packet(10, true, &[1; 3_000], 1_200).unwrap();
        let new = fragment_video_packet(11, false, &[2; 100], 1_200).unwrap();
        let now = Instant::now();
        let mut reassembler = VideoDatagramReassembler::default();
        assert!(reassembler.push(&old[0], now).unwrap().packet.is_none());
        let update = reassembler.push(&new[0], now).unwrap();
        assert_eq!(update.dropped_incomplete, 1);
        assert!(update.lost_keyframe);
        assert_eq!(update.packet, Some(vec![2; 100]));
        assert!(reassembler.push(&old[1], now).unwrap().packet.is_none());
    }

    #[test]
    fn video_reassembly_deadline_discards_late_fragments() {
        let fragments = fragment_video_packet(22, false, &[3; 3_000], 1_200).unwrap();
        let now = Instant::now();
        let mut reassembler = VideoDatagramReassembler::new(Duration::from_millis(20));
        assert!(reassembler
            .push(&fragments[0], now)
            .unwrap()
            .packet
            .is_none());
        let update = reassembler
            .push(&fragments[1], now + Duration::from_millis(21))
            .unwrap();
        assert_eq!(update.dropped_incomplete, 1);
        assert!(!update.lost_keyframe);
        assert!(update.packet.is_none());
    }

    #[test]
    fn v2_video_datagrams_use_rvd2_and_reassemble() {
        let packet = (0..5000u32).map(|value| value as u8).collect::<Vec<_>>();
        let fragments = fragment_video_packet_v2(42, true, &packet, 1200).unwrap();
        assert!(fragments
            .iter()
            .all(|datagram| &datagram[0..4] == b"RVD2" && datagram[4] == 2));
        let decoded = decode_video_datagram(&fragments[0]).unwrap();
        assert_eq!(decoded.version, DatagramVersion::V2);
        let mut reassembler = VideoDatagramReassembler::default();
        let now = Instant::now();
        let mut complete = None;
        for datagram in fragments.iter().rev() {
            complete = reassembler.push(datagram, now).unwrap().packet.or(complete);
        }
        assert_eq!(complete.unwrap(), packet);
        let mut bad = fragments[0].clone();
        bad[4] = 1;
        assert_eq!(
            decode_video_datagram(&bad).unwrap_err(),
            DatagramError::UnsupportedVersion(1)
        );
    }
}
