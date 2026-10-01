// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The client side of the presence datagram channel (`cua-presence/1`,
//! `libs/cua/proto/PRESENCE.md` section 6): cursors as unreliable,
//! latest-wins QUIC datagrams, with a reliable control stream for the slot
//! table and cursor targets. Registered as
//! [`cua_spaces::presence::PresenceDatagrams`] by [`register`].

/// The Cua Spaces presence datagram channel.
#[derive(Debug, Default, Clone, Copy)]
pub struct QuicPresence;

/// Registers [`QuicPresence`] for this process.
pub fn register() {
    cua_spaces::presence::register_presence_datagrams(std::sync::Arc::new(QuicPresence));
}

impl PresenceDatagrams for QuicPresence {
    fn connect<'a>(
        &'a self,
        space: &'a Space,
        info: &'a pb::PresenceDatagrams,
        timeout: Duration,
    ) -> BoxFuture<
        'a,
        Result<(
            std::sync::Arc<dyn CursorUplink>,
            mpsc::Receiver<PresenceEvent>,
        )>,
    > {
        Box::pin(async move {
            let (link, events) = connect(space, info, timeout).await?;
            Ok((link as std::sync::Arc<dyn CursorUplink>, events))
        })
    }
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_media_protocol::presence::{
    CursorDatagram, CursorRecord, PRESENCE_ALPN, PresenceControl, PresenceWindowRef, quantize,
    seq_newer,
};
use cua_spacesd_client::pb;
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::mpsc;

use cua_spaces::extension::BoxFuture;
use cua_spaces::presence::{
    Cursor, CursorShape, CursorUplink, PresenceDatagrams, PresenceEvent, ShapeSource, now_ms,
};
use cua_spaces::{Error, Result, Space};

type Target = (String, Option<String>);

#[derive(Default)]
struct Shared {
    slots: HashMap<u8, String>,
    targets: HashMap<u8, Target>,
    last_seq: HashMap<u8, u16>,
    you: Option<u8>,
}

/// An open channel. Dropping it closes the connection.
pub(crate) struct Link {
    connection: quinn::Connection,
    control: tokio::sync::Mutex<quinn::SendStream>,
    shared: Arc<Mutex<Shared>>,
    seq: AtomicU16,
    counter: AtomicU32,
    last_target: Mutex<Option<Target>>,
    _endpoint: quinn::Endpoint,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"bye");
    }
}

fn line(message: &PresenceControl) -> Vec<u8> {
    let mut v = serde_json::to_vec(message).unwrap_or_default();
    v.push(b'\n');
    v
}

/// Encodes one uplink datagram for `cursor` from `slot`.
pub(crate) fn uplink_datagram(slot: u8, seq: u16, counter: u32, cursor: &Cursor) -> Vec<u8> {
    CursorDatagram {
        flags: 0,
        tick: counter,
        server_time_us: 0,
        records: vec![CursorRecord {
            slot,
            seq,
            x: quantize(cursor.x),
            y: quantize(cursor.y),
            shape: 0,
            visible: cursor.visible,
            pressed: cursor.pressed,
            shape_source: 0,
        }],
    }
    .encode()
}

/// Turns received records into events: drops the caller's own slot,
/// unknown slots and stale sequences.
fn records_to_events(
    shared: &mut Shared,
    datagram: &CursorDatagram,
    received_ms: f64,
) -> Vec<PresenceEvent> {
    let mut out = Vec::new();
    for r in &datagram.records {
        if Some(r.slot) == shared.you {
            continue;
        }
        let Some(participant_id) = shared.slots.get(&r.slot).cloned() else {
            continue;
        };
        if let Some(last) = shared.last_seq.get(&r.slot)
            && !seq_newer(r.seq, *last)
        {
            continue;
        }
        shared.last_seq.insert(r.slot, r.seq);
        let (display_id, window_id) = shared.targets.get(&r.slot).cloned().unwrap_or_default();
        let (x, y) = r.position();
        out.push(PresenceEvent::CursorMoved {
            participant_id,
            cursor: Cursor {
                display_id,
                window_id,
                x,
                y,
                visible: r.visible,
                pressed: r.pressed,
                shape: CursorShape::from_wire(i32::from(r.shape)),
                shape_source: ShapeSource::from_wire(i32::from(r.shape_source)),
                at_ms: datagram.server_time_us as f64 / 1_000.0,
                received_ms,
            },
        });
    }
    out
}

fn apply_control(shared: &mut Shared, message: PresenceControl) {
    match message {
        PresenceControl::Slots { slots, you } => {
            shared.slots = slots
                .into_iter()
                .filter_map(|(k, v)| k.parse::<u8>().ok().map(|k| (k, v)))
                .collect();
            shared.you = Some(you);
            let live: Vec<u8> = shared.slots.keys().copied().collect();
            shared.last_seq.retain(|k, _| live.contains(k));
            shared.targets.retain(|k, _| live.contains(k));
        }
        PresenceControl::CursorTarget {
            slot,
            display_id,
            window,
        } => {
            shared
                .targets
                .insert(slot, (display_id, window.map(|w| w.id)));
        }
        PresenceControl::PresenceTicket { .. } => {}
    }
}

/// Opens the channel described by `info` and waits (up to `timeout`) for
/// the slot table. Returns the uplink and the stream of cursor events.
pub(crate) async fn connect(
    space: &Space,
    info: &pb::PresenceDatagrams,
    timeout: Duration,
) -> Result<(Arc<Link>, mpsc::Receiver<PresenceEvent>)> {
    let endpoint_info = info
        .endpoint
        .as_ref()
        .ok_or_else(|| Error::Stream("presence datagrams: no endpoint".into()))?;
    let pin = cua_media_transport::quic::parse_sha256(&endpoint_info.certificate_sha256)
        .ok_or_else(|| Error::Stream("presence datagrams: bad certificate pin".into()))?;
    let base = url::Url::parse(&space.websocket_url("/")?)
        .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?;
    let host = base
        .host_str()
        .ok_or_else(|| Error::Stream("presence datagrams: no host".into()))?
        .trim_matches(['[', ']'])
        .to_string();
    let port = u16::try_from(endpoint_info.port)
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| Error::Stream("presence datagrams: no port".into()))?;
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?
        .next()
        .ok_or_else(|| Error::Stream("presence datagrams: host did not resolve".into()))?;
    connect_addr(addr, pin, info, timeout).await
}

/// [`connect`] to a resolved address.
pub(crate) async fn connect_addr(
    addr: std::net::SocketAddr,
    pin: [u8; 32],
    info: &pb::PresenceDatagrams,
    timeout: Duration,
) -> Result<(Arc<Link>, mpsc::Receiver<PresenceEvent>)> {
    let endpoint_info = info.endpoint.clone().unwrap_or_default();
    let alpn = if endpoint_info.alpn.is_empty() {
        PRESENCE_ALPN.to_string()
    } else {
        endpoint_info.alpn.clone()
    };
    let bind: std::net::SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let config = cua_media_transport::quic::client_config_with_alpn(pin, alpn.as_bytes())
        .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?;
    let mut endpoint = quinn::Endpoint::client(bind)
        .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?;
    endpoint.set_default_client_config(config);
    let open = async {
        let connection = endpoint
            .connect(addr, "cua-spacesd.local")
            .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?
            .await
            .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?;
        let (mut send, recv) = connection
            .open_bi()
            .await
            .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?;
        send.write_all(&line(&PresenceControl::PresenceTicket {
            ticket: info.ticket.clone(),
        }))
        .await
        .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?;
        let mut lines = tokio::io::BufReader::new(recv).lines();
        let shared = Arc::new(Mutex::new(Shared::default()));
        // The slot table comes first; nothing can be sent before it.
        loop {
            let l = lines
                .next_line()
                .await
                .map_err(|e| Error::Stream(format!("presence datagrams: {e}")))?
                .ok_or_else(|| Error::Stream("presence datagrams: closed before slots".into()))?;
            if let Ok(m) = serde_json::from_str::<PresenceControl>(&l) {
                let is_slots = matches!(m, PresenceControl::Slots { .. });
                apply_control(&mut shared.lock().unwrap(), m);
                if is_slots {
                    break;
                }
            }
        }
        Ok::<_, Error>((connection, send, lines, shared))
    };
    let (connection, send, mut lines, shared) = tokio::time::timeout(timeout, open)
        .await
        .map_err(|_| Error::Timeout("presence datagram channel".into()))??;

    let (tx, rx) = mpsc::channel(1024);
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            // Bounded by the connection: ends when the stream closes.
            while let Ok(Some(l)) = lines.next_line().await {
                if let Ok(m) = serde_json::from_str::<PresenceControl>(&l) {
                    apply_control(&mut shared.lock().unwrap(), m);
                }
            }
        });
    }
    {
        let shared = shared.clone();
        let connection = connection.clone();
        tokio::spawn(async move {
            // Ends when the connection closes (read_datagram errors).
            while let Ok(bytes) = connection.read_datagram().await {
                let Ok(datagram) = CursorDatagram::decode(&bytes) else {
                    continue;
                };
                let events = records_to_events(&mut shared.lock().unwrap(), &datagram, now_ms());
                for e in events {
                    // Latest wins: a full queue drops, never blocks.
                    if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(e) {
                        return;
                    }
                }
            }
        });
    }
    Ok((
        Arc::new(Link {
            connection,
            control: tokio::sync::Mutex::new(send),
            shared,
            seq: AtomicU16::new(0),
            counter: AtomicU32::new(0),
            last_target: Mutex::new(None),
            _endpoint: endpoint,
        }),
        rx,
    ))
}

#[async_trait::async_trait]
impl CursorUplink for Link {
    async fn send(&self, cursor: &Cursor) -> Result<bool> {
        if self.connection.close_reason().is_some() {
            return Ok(false);
        }
        let Some(slot) = self.shared.lock().unwrap().you else {
            return Ok(false);
        };
        let target = (cursor.display_id.clone(), cursor.window_id.clone());
        let changed = self.last_target.lock().unwrap().as_ref() != Some(&target);
        if changed {
            let message = PresenceControl::CursorTarget {
                slot,
                display_id: target.0.clone(),
                window: target
                    .1
                    .clone()
                    .map(|id| PresenceWindowRef { id, epoch: 0 }),
            };
            let mut control = self.control.lock().await;
            if control.write_all(&line(&message)).await.is_err() {
                return Ok(false);
            }
            *self.last_target.lock().unwrap() = Some(target);
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let counter = self.counter.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let bytes = uplink_datagram(slot, seq, counter, cursor);
        Ok(self.connection.send_datagram(bytes.into()).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole channel against a loopback QUIC server speaking
    /// PRESENCE.md section 6: ticket, slots, a downlink batch, an uplink
    /// cursor with its target line.
    #[tokio::test]
    async fn connects_receives_and_sends_over_loopback_quic() {
        let identity = cua_media_transport::quic::QuicIdentity::generate().unwrap();
        let server_config = identity
            .server_config_with_alpns(&[PRESENCE_ALPN.as_bytes()])
            .unwrap();
        let server =
            quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = server.local_addr().unwrap();
        let pin = cua_media_transport::quic::parse_sha256(&identity.certificate_sha256()).unwrap();
        let serve = tokio::spawn(async move {
            let conn = server.accept().await.unwrap().await.unwrap();
            let (mut send, recv) = conn.accept_bi().await.unwrap();
            let mut lines = tokio::io::BufReader::new(recv).lines();
            let first = lines.next_line().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<PresenceControl>(&first).unwrap(),
                PresenceControl::PresenceTicket {
                    ticket: "tkt".into()
                }
            );
            send.write_all(
                b"{\"type\":\"slots\",\"payload\":{\"slots\":{\"1\":\"p-me\",\"2\":\"p-bob\"},\"you\":1}}\n",
            )
            .await
            .unwrap();
            // Downlink: bob over a text field.
            let down = CursorDatagram {
                flags: 0,
                tick: 1,
                server_time_us: 7_000,
                records: vec![CursorRecord {
                    slot: 2,
                    seq: 1,
                    x: quantize(0.5),
                    y: quantize(0.25),
                    shape: 2,
                    visible: true,
                    pressed: false,
                    shape_source: 1,
                }],
            };
            // Datagrams are unreliable: repeat until the client answers.
            let uplink = async {
                loop {
                    conn.send_datagram(down.encode().into()).unwrap();
                    if let Ok(Ok(bytes)) =
                        tokio::time::timeout(Duration::from_millis(50), conn.read_datagram()).await
                    {
                        return bytes;
                    }
                }
            };
            let bytes = tokio::time::timeout(Duration::from_secs(5), uplink)
                .await
                .unwrap();
            let target = lines.next_line().await.unwrap().unwrap();
            (CursorDatagram::decode(&bytes).unwrap(), target, conn)
        });
        let info = pb::PresenceDatagrams {
            endpoint: Some(pb::QuicEndpoint {
                port: u32::from(addr.port()),
                certificate_sha256: identity.certificate_sha256(),
                alpn: PRESENCE_ALPN.into(),
            }),
            ticket: "tkt".into(),
            tick: None,
        };
        let (link, mut events) = connect_addr(addr, pin, &info, Duration::from_secs(5))
            .await
            .unwrap();
        let e = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        let PresenceEvent::CursorMoved {
            participant_id,
            cursor,
        } = e
        else {
            panic!("{e:?}")
        };
        assert_eq!(participant_id, "p-bob");
        assert_eq!(cursor.shape, CursorShape::Text);
        assert_eq!(cursor.at_ms, 7.0);
        let mut mine = Cursor::at(0.75, 0.5);
        mine.window_id = Some("w3".into());
        // Keep sending until the server has one (datagrams may drop).
        let sent = tokio::spawn({
            let link = link.clone();
            async move {
                for _ in 0..100 {
                    assert!(link.send(&mine).await.unwrap());
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        });
        let (up, target, _conn) = serve.await.unwrap();
        sent.abort();
        assert_eq!(up.records[0].slot, 1);
        assert!((up.records[0].position().0 - 0.75).abs() < 1e-4);
        assert_eq!(
            serde_json::from_str::<PresenceControl>(&target).unwrap(),
            PresenceControl::CursorTarget {
                slot: 1,
                display_id: String::new(),
                window: Some(PresenceWindowRef {
                    id: "w3".into(),
                    epoch: 0
                }),
            }
        );
    }

    fn shared() -> Shared {
        let mut s = Shared::default();
        apply_control(
            &mut s,
            serde_json::from_str(
                r#"{"type":"slots","payload":{"slots":{"1":"p-me","2":"p-bob"},"you":1}}"#,
            )
            .unwrap(),
        );
        apply_control(
            &mut s,
            PresenceControl::CursorTarget {
                slot: 2,
                display_id: String::new(),
                window: Some(PresenceWindowRef {
                    id: "w7".into(),
                    epoch: 0,
                }),
            },
        );
        s
    }

    fn record(slot: u8, seq: u16, x: f64) -> CursorRecord {
        CursorRecord {
            slot,
            seq,
            x: quantize(x),
            y: quantize(0.5),
            shape: 2,
            visible: true,
            pressed: false,
            shape_source: 1,
        }
    }

    #[test]
    fn records_become_cursor_events_with_shape_target_and_server_time() {
        let mut s = shared();
        let d = CursorDatagram {
            flags: 0,
            tick: 1,
            server_time_us: 5_000_000,
            records: vec![record(1, 1, 0.1), record(2, 1, 0.25), record(9, 1, 0.3)],
        };
        let bytes = d.encode();
        let events = records_to_events(&mut s, &CursorDatagram::decode(&bytes).unwrap(), 42.0);
        assert_eq!(events.len(), 1, "own slot and unknown slots are dropped");
        let PresenceEvent::CursorMoved {
            participant_id,
            cursor,
        } = &events[0]
        else {
            panic!()
        };
        assert_eq!(participant_id, "p-bob");
        assert_eq!(cursor.window_id.as_deref(), Some("w7"));
        assert_eq!(cursor.shape, CursorShape::Text);
        assert_eq!(cursor.shape_source, ShapeSource::HitTest);
        assert_eq!(cursor.at_ms, 5_000.0);
        assert_eq!(cursor.received_ms, 42.0);
        assert!((cursor.x - 0.25).abs() < 1e-4);
    }

    #[test]
    fn stale_and_duplicate_sequences_are_dropped_across_wrap() {
        let mut s = shared();
        let mk = |seq, x| CursorDatagram {
            flags: 0,
            tick: 1,
            server_time_us: 1,
            records: vec![record(2, seq, x)],
        };
        assert_eq!(records_to_events(&mut s, &mk(65535, 0.1), 0.0).len(), 1);
        assert_eq!(
            records_to_events(&mut s, &mk(65535, 0.2), 0.0).len(),
            0,
            "duplicate"
        );
        assert_eq!(
            records_to_events(&mut s, &mk(0, 0.3), 0.0).len(),
            1,
            "wraps"
        );
        assert_eq!(
            records_to_events(&mut s, &mk(65534, 0.4), 0.0).len(),
            0,
            "stale"
        );
    }

    #[test]
    fn uplink_is_one_record_from_our_slot() {
        let mut c = Cursor::at(0.5, 0.75);
        c.pressed = true;
        let d = CursorDatagram::decode(&uplink_datagram(3, 7, 9, &c)).unwrap();
        assert_eq!(d.records.len(), 1);
        let r = d.records[0];
        assert_eq!((r.slot, r.seq, r.shape, r.shape_source), (3, 7, 0, 0));
        assert!(r.visible && r.pressed);
        assert_eq!(d.server_time_us, 0);
    }
}
