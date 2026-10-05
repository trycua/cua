// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The daemon's test fixtures (open source) frame the media wire by hand;
//! this pins them to the driver's own protocol crates.

use cua_daemon::fixtures::{v2, video_packet};

#[test]
fn media_wire_matches_the_driver() {
    assert_eq!(v2::WS_SUBPROTOCOL, cua_media_protocol::v2::WS_SUBPROTOCOL);
    assert_eq!(
        v2::WS_TICKET_SUBPROTOCOL_PREFIX,
        cua_media_protocol::v2::WS_TICKET_SUBPROTOCOL_PREFIX
    );
    assert_eq!(
        v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX,
        cua_media_protocol::v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX
    );
    for (sequence, keyframe) in [(7, true), (8, false)] {
        let packet = video_packet(sequence, keyframe);
        let (header, payload) = cua_media_transport::decode_packet(&packet).unwrap();
        let cua_media_protocol::WireHeader::Video(d) = &header else {
            panic!("not a video header: {header:?}");
        };
        assert_eq!(d.sequence.0, sequence);
        assert_eq!(d.keyframe, keyframe);
        assert_eq!(d.codec, cua_media_protocol::VideoCodec::H264);
        // Byte for byte what the driver's encoder writes.
        assert_eq!(
            cua_media_transport::encode_packet(&header, &payload).unwrap(),
            packet
        );
    }
}
