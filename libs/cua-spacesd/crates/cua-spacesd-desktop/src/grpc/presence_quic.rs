// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The presence datagram channel server (`cua-presence/1`, PRESENCE.md
//! section 6) on the QUIC media listener.
//!
//! Per connection: one bidirectional stream carrying JSON lines (the ticket
//! first, then `slots` and `cursor_target` from the server and
//! `cursor_target` from the client), and `RPC1` datagrams both ways. The
//! server sends every changed cursor each [`DATAGRAM_TICK`] and the full set
//! every [`KEYFRAME_INTERVAL`]. While the channel is attached the `Join`
//! stream stops carrying cursor moves for that participant.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use cua_media_protocol::presence::{
    quantize, CursorDatagram, CursorRecord, PresenceControl, PresenceWindowRef, FLAG_KEYFRAME,
};
use cua_proto::env::v1::WindowRef;
use tokio::io::AsyncReadExt;

use super::presence::{DatagramView, PresenceHub};

/// Server cursor tick on the channel (30 Hz).
pub const DATAGRAM_TICK: Duration = Duration::from_millis(33);
/// Full cursor set this often, so a lost final update heals.
pub const KEYFRAME_INTERVAL: Duration = Duration::from_secs(1);
/// Longest accepted control line.
const MAX_LINE: usize = 16 * 1024;
/// QUIC application error: bad or used ticket.
const TICKET_INVALID: u32 = 0x401;
/// QUIC application error: the participant left presence.
const GONE: u32 = 0x410;

fn server_time_us() -> u64 {
    micros(SystemTime::now())
}

fn micros(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Reads newline-delimited control lines from a stream, bounded.
struct Lines<R> {
    reader: R,
    buffer: Vec<u8>,
}

impl<R: tokio::io::AsyncRead + Unpin> Lines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
        }
    }

    /// The next line, `Ok(None)` at end of stream.
    async fn next(&mut self) -> Result<Option<String>, String> {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                return Ok(Some(String::from_utf8_lossy(&line[..end]).into_owned()));
            }
            if self.buffer.len() > MAX_LINE {
                return Err("control line too long".into());
            }
            let mut chunk = [0u8; 4096];
            let read = self
                .reader
                .read(&mut chunk)
                .await
                .map_err(|error| error.to_string())?;
            if read == 0 {
                return Ok(None);
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

async fn send_line(send: &mut quinn::SendStream, message: &PresenceControl) -> Result<(), String> {
    let mut line = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    line.push(b'\n');
    send.write_all(&line)
        .await
        .map_err(|error| error.to_string())
}

fn target_of(cursor: &cua_proto::env::v1::CursorPosition) -> (String, Option<PresenceWindowRef>) {
    (
        cursor.display_id.clone(),
        cursor.window.as_ref().map(|window| PresenceWindowRef {
            id: window.id.clone(),
            epoch: window.epoch,
        }),
    )
}

/// What one connection has already told its client.
#[derive(Default)]
struct Sent {
    slot_generation: Option<u64>,
    seqs: HashMap<u8, u16>,
    targets: HashMap<u8, (String, Option<PresenceWindowRef>)>,
    tick: u32,
    last_keyframe: Option<Instant>,
}

impl Sent {
    /// Control lines and the datagram for one tick of `view`.
    fn step(
        &mut self,
        view: &DatagramView,
        now: Instant,
    ) -> (Vec<PresenceControl>, Option<CursorDatagram>) {
        let mut lines = Vec::new();
        if self.slot_generation != Some(view.slot_generation) {
            self.slot_generation = Some(view.slot_generation);
            lines.push(PresenceControl::Slots {
                slots: view
                    .slots
                    .iter()
                    .map(|(slot, id)| (slot.to_string(), id.clone()))
                    .collect(),
                you: view.you,
            });
            // Slots may have been reused: forget per-slot state.
            self.seqs.retain(|slot, _| view.slots.contains_key(slot));
            self.targets.retain(|slot, _| view.slots.contains_key(slot));
        }
        let keyframe = self
            .last_keyframe
            .is_none_or(|last| now.duration_since(last) >= KEYFRAME_INTERVAL);
        let mut records = Vec::new();
        let mut newest: Option<SystemTime> = None;
        for cursor in &view.cursors {
            let target = target_of(&cursor.cursor);
            if self.targets.get(&cursor.slot) != Some(&target) {
                lines.push(PresenceControl::CursorTarget {
                    slot: cursor.slot,
                    display_id: target.0.clone(),
                    window: target.1.clone(),
                });
                self.targets.insert(cursor.slot, target);
            }
            if !keyframe && self.seqs.get(&cursor.slot) == Some(&cursor.seq) {
                continue;
            }
            self.seqs.insert(cursor.slot, cursor.seq);
            newest = newest.max(Some(cursor.received));
            let point = cursor.cursor.position.unwrap_or_default();
            records.push(CursorRecord {
                slot: cursor.slot,
                seq: cursor.seq,
                x: quantize(point.x),
                y: quantize(point.y),
                shape: u8::try_from(cursor.shape).unwrap_or(0),
                visible: cursor.cursor.visible,
                pressed: false,
                shape_source: u8::try_from(cursor.shape_source).unwrap_or(0) & 0b11,
            });
        }
        if keyframe {
            self.last_keyframe = Some(now);
        }
        self.tick = self.tick.wrapping_add(1);
        if records.is_empty() && !keyframe {
            return (lines, None);
        }
        (
            lines,
            Some(CursorDatagram {
                flags: if keyframe { FLAG_KEYFRAME } else { 0 },
                tick: self.tick,
                // When the server received the newest record: exact for the
                // cursor that moved (receivers time their interpolation by
                // it), at most a tick early for the others.
                server_time_us: newest.map(micros).unwrap_or_else(server_time_us),
                records,
            }),
        )
    }
}

/// Serve one presence connection.
pub(crate) async fn serve(
    hub: Arc<PresenceHub>,
    connection: quinn::Connection,
) -> Result<(), String> {
    let (mut send, receive) = tokio::time::timeout(Duration::from_secs(10), connection.accept_bi())
        .await
        .map_err(|_| "no stream")?
        .map_err(|error| error.to_string())?;
    let mut lines = Lines::new(receive);
    let first = tokio::time::timeout(Duration::from_secs(10), lines.next())
        .await
        .map_err(|_| "no ticket")??;
    let participant =
        match first.and_then(|line| serde_json::from_str::<PresenceControl>(&line).ok()) {
            Some(PresenceControl::PresenceTicket { ticket }) => hub.attach_datagram(&ticket),
            _ => None,
        };
    let Some(participant) = participant else {
        connection.close(TICKET_INVALID.into(), b"ticket");
        return Err("invalid presence ticket".into());
    };
    let result = run(&hub, &connection, &mut send, &mut lines, &participant).await;
    hub.detach_datagram(&participant);
    result
}

async fn run(
    hub: &Arc<PresenceHub>,
    connection: &quinn::Connection,
    send: &mut quinn::SendStream,
    lines: &mut Lines<quinn::RecvStream>,
    participant: &str,
) -> Result<(), String> {
    let mut sent = Sent::default();
    let mut tick = tokio::time::interval(DATAGRAM_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut you = 0u8;
    let mut last_send: Option<Instant> = None;
    loop {
        // Leading edge: a change sends at once when a tick has passed since
        // the last datagram, so a lone move is not held for up to a tick and
        // `server_time_us` stays close to when the server got it. The tick
        // sends whatever arrived inside the interval, and keyframes.
        let changed = hub.cursor_changed();
        // Three quarters of a tick, so a steady 30 Hz sender's jitter does
        // not push every other move to the next tick.
        let due = last_send.is_none_or(|at| at.elapsed() >= DATAGRAM_TICK * 3 / 4);
        tokio::select! {
            _ = tick.tick() => {}
            _ = changed, if due => {}
            datagram = connection.read_datagram() => {
                let bytes = datagram.map_err(|error| error.to_string())?;
                let Ok(datagram) = CursorDatagram::decode(&bytes) else {
                    continue;
                };
                for record in datagram.records.iter().filter(|record| record.slot == you && you != 0) {
                    hub.datagram_uplink(participant, record.seq, record.position(), record.visible);
                }
                continue;
            }
            line = lines.next() => {
                let Some(line) = line? else {
                    return Ok(());
                };
                if let Ok(PresenceControl::CursorTarget { slot, display_id, window }) =
                    serde_json::from_str::<PresenceControl>(&line)
                {
                    if slot == you {
                        hub.datagram_target(
                            participant,
                            display_id,
                            window.map(|window| WindowRef { id: window.id, epoch: window.epoch }),
                        );
                    }
                }
                continue;
            }
        }
        let Some(view) = hub.datagram_view(participant) else {
            connection.close(GONE.into(), b"left");
            return Ok(());
        };
        you = view.you;
        let (controls, datagram) = sent.step(&view, Instant::now());
        for control in &controls {
            send_line(send, control).await?;
        }
        if let Some(datagram) = datagram {
            last_send = Some(Instant::now());
            for part in datagram.split() {
                // Latest wins: a full buffer drops, never waits.
                let _ = connection.send_datagram(part.encode().into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_protocol::presence::PRESENCE_ALPN;
    use cua_media_transport::quic::{client_config_with_alpn, parse_sha256, QuicIdentity};
    use cua_proto::env::v1::{CursorPosition, Point, Principal};

    fn at(x: f64, y: f64) -> CursorPosition {
        CursorPosition {
            display_id: "0".into(),
            position: Some(Point { x, y }),
            visible: true,
            ..CursorPosition::default()
        }
    }

    /// A loopback listener that serves only the presence ALPN.
    async fn listener(hub: Arc<PresenceHub>) -> (std::net::SocketAddr, String) {
        let identity = QuicIdentity::generate().unwrap();
        let config = identity
            .server_config_with_alpns(&[
                cua_media_protocol::v2::QUIC_ALPN,
                PRESENCE_ALPN.as_bytes(),
            ])
            .unwrap();
        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = endpoint.local_addr().unwrap();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let hub = hub.clone();
                tokio::spawn(async move {
                    if let Ok(connection) = incoming.await {
                        let _ = serve(hub, connection).await;
                    }
                });
            }
        });
        (addr, identity.certificate_sha256())
    }

    async fn connect(addr: std::net::SocketAddr, pin: &str) -> quinn::Connection {
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(
            client_config_with_alpn(parse_sha256(pin).unwrap(), PRESENCE_ALPN.as_bytes()).unwrap(),
        );
        endpoint
            .connect(addr, "cua-spacesd.local")
            .unwrap()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn datagrams_carry_cursors_both_ways() {
        let hub = PresenceHub::new();
        let (alice, _, _alice_rx) = hub.join(Principal {
            id: "alice".into(),
            ..Principal::default()
        });
        let (bob, _, _bob_rx) = hub.join(Principal {
            id: "bob".into(),
            ..Principal::default()
        });
        hub.update_cursor(&alice.participant_id, at(0.25, 0.75));
        let (addr, pin) = listener(hub.clone()).await;
        let connection = connect(addr, &pin).await;
        let (mut send, receive) = connection.open_bi().await.unwrap();
        let ticket = hub.mint_datagram_ticket(&bob.participant_id);
        send_line(&mut send, &PresenceControl::PresenceTicket { ticket })
            .await
            .unwrap();
        let mut lines = Lines::new(receive);
        let slots = tokio::time::timeout(Duration::from_secs(5), lines.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let PresenceControl::Slots { slots, you } = serde_json::from_str(&slots).unwrap() else {
            panic!("first line is the slot table");
        };
        assert_eq!(slots.len(), 2);
        let alice_slot: u8 = slots
            .iter()
            .find(|(_, id)| **id == alice.participant_id)
            .unwrap()
            .0
            .parse()
            .unwrap();
        // Alice's cursor arrives as a datagram.
        let mut got = None;
        for _ in 0..20 {
            let bytes = tokio::time::timeout(Duration::from_secs(5), connection.read_datagram())
                .await
                .unwrap()
                .unwrap();
            let datagram = CursorDatagram::decode(&bytes).unwrap();
            if let Some(record) = datagram.records.iter().find(|r| r.slot == alice_slot) {
                got = Some(*record);
                break;
            }
        }
        let (x, y) = got.expect("alice's record").position();
        assert!((x - 0.25).abs() < 1e-4 && (y - 0.75).abs() < 1e-4);
        // Bob moves over the channel; the hub sees it (Alice's view).
        let uplink = CursorDatagram {
            flags: 0,
            tick: 1,
            server_time_us: 0,
            records: vec![CursorRecord {
                slot: you,
                seq: 1,
                x: quantize(0.5),
                y: quantize(0.125),
                shape: 0,
                visible: true,
                pressed: false,
                shape_source: 0,
            }],
        };
        connection.send_datagram(uplink.encode().into()).unwrap();
        let mut seen = false;
        for _ in 0..100 {
            if let Some(view) = hub.datagram_view(&alice.participant_id) {
                if view.cursors.iter().any(|c| {
                    c.slot == you && (c.cursor.position.as_ref().unwrap().y - 0.125).abs() < 1e-4
                }) {
                    seen = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(seen, "the uplink record was applied");
        // Leading edge: after a quiet period, a lone move goes out at once
        // rather than at the next 33 ms tick (loopback: well under a tick).
        tokio::time::sleep(Duration::from_millis(120)).await;
        while tokio::time::timeout(Duration::from_millis(5), connection.read_datagram())
            .await
            .is_ok()
        {}
        let mut lags = Vec::new();
        for i in 0..5u32 {
            let x = 0.1 + f64::from(i) * 0.1;
            let moved = Instant::now();
            hub.update_cursor(&alice.participant_id, at(x, 0.5));
            for _ in 0..20 {
                let bytes =
                    tokio::time::timeout(Duration::from_secs(2), connection.read_datagram())
                        .await
                        .unwrap()
                        .unwrap();
                let d = CursorDatagram::decode(&bytes).unwrap();
                if d.records
                    .iter()
                    .any(|r| r.slot == alice_slot && (r.position().0 - x).abs() < 1e-3)
                {
                    lags.push(moved.elapsed());
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        assert_eq!(lags.len(), 5);
        let worst = lags.iter().max().unwrap();
        assert!(
            *worst < Duration::from_millis(25),
            "leading-edge sends: {lags:?}"
        );
        connection.close(0u32.into(), b"");
    }

    #[tokio::test]
    async fn a_bad_ticket_closes_the_connection() {
        let hub = PresenceHub::new();
        let (addr, pin) = listener(hub.clone()).await;
        let connection = connect(addr, &pin).await;
        let (mut send, _receive) = connection.open_bi().await.unwrap();
        send_line(
            &mut send,
            &PresenceControl::PresenceTicket {
                ticket: "nope".into(),
            },
        )
        .await
        .unwrap();
        let reason = tokio::time::timeout(Duration::from_secs(5), connection.closed())
            .await
            .unwrap();
        assert!(
            matches!(reason, quinn::ConnectionError::ApplicationClosed(ref close) if close.error_code == TICKET_INVALID.into()),
            "{reason:?}"
        );
    }

    #[test]
    fn a_tick_sends_only_changes_and_keyframes_every_second() {
        let view = |seq: u16| DatagramView {
            you: 1,
            slot_generation: 1,
            slots: [(1, "me".to_string()), (2, "them".to_string())].into(),
            cursors: vec![super::super::presence::DatagramCursor {
                slot: 2,
                seq,
                cursor: at(0.5, 0.5),
                shape: 2,
                shape_source: 1,
                received: SystemTime::UNIX_EPOCH + Duration::from_micros(1_234_567),
            }],
        };
        let mut sent = Sent::default();
        let start = Instant::now();
        let (lines, first) = sent.step(&view(1), start);
        assert_eq!(
            first.as_ref().unwrap().server_time_us,
            1_234_567,
            "stamped with the record's receive time"
        );
        assert_eq!(lines.len(), 2, "slots and the target");
        assert_eq!(first.unwrap().flags, FLAG_KEYFRAME);
        let (lines, same) = sent.step(&view(1), start + Duration::from_millis(33));
        assert!(lines.is_empty() && same.is_none(), "nothing changed");
        let (_, moved) = sent.step(&view(2), start + Duration::from_millis(66));
        let moved = moved.unwrap();
        assert_eq!(moved.flags, 0);
        assert_eq!(moved.records[0].shape, 2);
        let (_, key) = sent.step(&view(2), start + Duration::from_millis(1100));
        assert_eq!(key.unwrap().records.len(), 1, "keyframe resends");
    }
}
