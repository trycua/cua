// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Reverse-SOCKS egress ("hotspot") for cua-spacesd.
//!
//! A SOCKS5 listener on guest loopback relays each outbound connection over a
//! single message channel (a WebSocket in practice) to a *peer* that performs
//! the real egress from its own network. The WebSocket itself is served and
//! authenticated by cua-spacesd (`/hotspot`, ticket-authenticated); this
//! crate has no network listener of its own besides the loopback SOCKS port.
//!
//! The wire protocol (one tunnel frame per binary message, see [`frame`])
//! and the peer side ([`peer::serve_egress`], used by the SDK and tests) live
//! in the shared `cua-hotspot` crate in libs/cua; this crate is only the
//! listener (the SOCKS server and its [`Hub`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

pub use cua_hotspot::{frame, peer, Frame};
use cua_hotspot::{CHANNEL_DEPTH, READ_CHUNK};

/// Where a SOCKS `CONNECT` goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Through the connected peer (refused when no peer is connected).
    Peer,
    /// Directly from this host (bypass list).
    Direct,
    /// Refuse.
    Reject,
}

/// Routing policy for SOCKS requests.
pub type Policy = Arc<dyn Fn(&str, u16) -> Route + Send + Sync>;

/// Policy sending everything through the peer.
pub fn peer_only() -> Policy {
    Arc::new(|_, _| Route::Peer)
}

/// Counters of one hub.
#[derive(Debug, Default)]
pub struct Stats {
    /// Open relayed connections.
    pub active_connections: AtomicU32,
    /// Bytes sent from the guest through the peer.
    pub bytes_out: AtomicU64,
    /// Bytes received into the guest through the peer.
    pub bytes_in: AtomicU64,
}

enum StreamMsg {
    OpenOk,
    OpenErr,
    Data(Vec<u8>),
    Close,
}

struct Tunnel {
    to_peer: mpsc::Sender<Frame>,
    streams: Mutex<HashMap<u32, mpsc::Sender<StreamMsg>>>,
    next_stream: AtomicU32,
}

impl Tunnel {
    fn register(&self) -> (u32, mpsc::Receiver<StreamMsg>) {
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        self.streams.lock().expect("streams").insert(id, tx);
        (id, rx)
    }

    fn drop_stream(&self, id: u32) {
        self.streams.lock().expect("streams").remove(&id);
    }
}

/// Holds the current peer (the newest attached peer wins).
#[derive(Clone, Default)]
pub struct Hub {
    current: Arc<Mutex<Option<Arc<Tunnel>>>>,
    stats: Arc<Stats>,
}

impl Hub {
    /// Creates an empty hub.
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a peer is attached.
    pub fn is_connected(&self) -> bool {
        self.current.lock().expect("hub").is_some()
    }

    /// Counters.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    fn tunnel(&self) -> Option<Arc<Tunnel>> {
        self.current.lock().expect("hub").clone()
    }

    /// Serves one attached peer until its channel closes. `source` yields
    /// binary messages from the peer; `sink` sends binary messages to it.
    pub async fn run_peer<S, K>(&self, mut source: S, mut sink: K)
    where
        S: Stream<Item = Vec<u8>> + Unpin + Send,
        K: Sink<Vec<u8>> + Unpin + Send + 'static,
    {
        let (to_peer, mut peer_rx) = mpsc::channel::<Frame>(CHANNEL_DEPTH);
        let tunnel = Arc::new(Tunnel {
            to_peer,
            streams: Mutex::new(HashMap::new()),
            next_stream: AtomicU32::new(1),
        });
        *self.current.lock().expect("hub") = Some(tunnel.clone());
        tracing::info!("hotspot peer attached");
        let writer = tokio::spawn(async move {
            while let Some(frame) = peer_rx.recv().await {
                if sink.send(frame.encode()).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        while let Some(message) = source.next().await {
            let Ok(frame) = Frame::decode(&message) else {
                continue;
            };
            let (stream, msg) = match frame {
                Frame::OpenOk { stream } => (stream, StreamMsg::OpenOk),
                Frame::OpenErr { stream, .. } => (stream, StreamMsg::OpenErr),
                Frame::Data { stream, bytes } => (stream, StreamMsg::Data(bytes)),
                Frame::Close { stream } => (stream, StreamMsg::Close),
                Frame::Open { .. } => continue,
            };
            let sender = tunnel
                .streams
                .lock()
                .expect("streams")
                .get(&stream)
                .cloned();
            if let Some(sender) = sender {
                // Backpressure instead of dropping data for a slow client.
                let _ = sender.send(msg).await;
            }
        }
        {
            let mut current = self.current.lock().expect("hub");
            if current.as_ref().is_some_and(|a| Arc::ptr_eq(a, &tunnel)) {
                *current = None;
            }
        }
        tunnel.streams.lock().expect("streams").clear();
        writer.abort();
        tracing::info!("hotspot peer detached");
    }
}

/// Accepts SOCKS5 clients on `listener` until it fails.
pub async fn serve_socks(listener: TcpListener, hub: Hub, policy: Policy) -> std::io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        let hub = hub.clone();
        let policy = policy.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_socks(stream, hub, policy).await {
                tracing::debug!(%error, "socks connection ended");
            }
        });
    }
}

async fn handle_socks(mut client: TcpStream, hub: Hub, policy: Policy) -> std::io::Result<()> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Ok(());
    }
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    client.write_all(&[0x05, 0x00]).await?;

    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[1] != 0x01 {
        return write_socks_reply(&mut client, 0x07).await;
    }
    let host = match req[3] {
        0x01 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        0x04 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => return write_socks_reply(&mut client, 0x08).await,
    };
    let mut port_bytes = [0u8; 2];
    client.read_exact(&mut port_bytes).await?;
    let port = u16::from_be_bytes(port_bytes);

    match policy(&host, port) {
        Route::Reject => write_socks_reply(&mut client, 0x02).await,
        Route::Direct => fulfill_direct(client, host, port).await,
        Route::Peer => match hub.tunnel() {
            Some(tunnel) => fulfill_via_tunnel(client, tunnel, hub.stats.clone(), host, port).await,
            // Never fall back to direct egress: the point of the hotspot is
            // that traffic leaves through the peer.
            None => write_socks_reply(&mut client, 0x03).await,
        },
    }
}

async fn fulfill_direct(mut client: TcpStream, host: String, port: u16) -> std::io::Result<()> {
    match TcpStream::connect((host.as_str(), port)).await {
        Ok(mut target) => {
            write_socks_reply(&mut client, 0x00).await?;
            let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
            Ok(())
        }
        Err(_) => write_socks_reply(&mut client, 0x05).await,
    }
}

/// Relays every connection accepted on `listener` to the peer as a stream
/// to the fixed target `host:port` (no SOCKS handshake): the guest mount of
/// Cua Volume dials its loopback listener, and the peer (the host serving
/// the volume) answers `host`. A connection made while no peer is attached
/// is closed.
pub async fn serve_forward(
    listener: TcpListener,
    hub: Hub,
    host: String,
    port: u16,
) -> std::io::Result<()> {
    loop {
        let (client, _peer) = listener.accept().await?;
        let Some(tunnel) = hub.tunnel() else {
            drop(client);
            continue;
        };
        let (stats, host) = (hub.stats.clone(), host.clone());
        tokio::spawn(async move {
            if let Err(error) = relay(client, tunnel, stats, host, port, false).await {
                tracing::debug!(%error, "forwarded connection ended");
            }
        });
    }
}

async fn fulfill_via_tunnel(
    client: TcpStream,
    tunnel: Arc<Tunnel>,
    stats: Arc<Stats>,
    host: String,
    port: u16,
) -> std::io::Result<()> {
    relay(client, tunnel, stats, host, port, true).await
}

/// One stream through the peer; `socks` writes the SOCKS replies.
async fn relay(
    mut client: TcpStream,
    tunnel: Arc<Tunnel>,
    stats: Arc<Stats>,
    host: String,
    port: u16,
    socks: bool,
) -> std::io::Result<()> {
    let (id, mut from_peer) = tunnel.register();
    if tunnel
        .to_peer
        .send(Frame::Open {
            stream: id,
            host,
            port,
        })
        .await
        .is_err()
    {
        tunnel.drop_stream(id);
        return if socks {
            write_socks_reply(&mut client, 0x03).await
        } else {
            Ok(())
        };
    }
    match from_peer.recv().await {
        Some(StreamMsg::OpenOk) => {
            if socks {
                write_socks_reply(&mut client, 0x00).await?
            }
        }
        _ => {
            tunnel.drop_stream(id);
            return if socks {
                write_socks_reply(&mut client, 0x05).await
            } else {
                Ok(())
            };
        }
    }
    stats.active_connections.fetch_add(1, Ordering::SeqCst);
    let (mut reader, mut writer) = client.into_split();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        tokio::select! {
            read = reader.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    stats.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
                    if tunnel.to_peer.send(Frame::Data { stream: id, bytes: buf[..n].to_vec() }).await.is_err() {
                        break;
                    }
                }
            },
            msg = from_peer.recv() => match msg {
                Some(StreamMsg::Data(bytes)) => {
                    stats.bytes_in.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    if writer.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Some(StreamMsg::Close) | None => break,
                Some(_) => {}
            },
        }
    }
    stats.active_connections.fetch_sub(1, Ordering::SeqCst);
    let _ = tunnel.to_peer.send(Frame::Close { stream: id }).await;
    tunnel.drop_stream(id);
    Ok(())
}

async fn write_socks_reply(client: &mut TcpStream, rep: u8) -> std::io::Result<()> {
    client
        .write_all(&[0x05, rep, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::time::Duration;

    async fn echo_server() -> std::net::SocketAddr {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let mut b = [0u8; 1024];
                    loop {
                        match s.read(&mut b).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if s.write_all(&b[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    async fn socks_connect(
        socks: std::net::SocketAddr,
        target: std::net::SocketAddr,
    ) -> (TcpStream, u8) {
        let mut c = TcpStream::connect(socks).await.unwrap();
        c.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut greet = [0u8; 2];
        c.read_exact(&mut greet).await.unwrap();
        let IpAddr::V4(ip) = target.ip() else {
            unreachable!()
        };
        let mut req = vec![0x05, 0x01, 0x00, 0x01];
        req.extend_from_slice(&ip.octets());
        req.extend_from_slice(&target.port().to_be_bytes());
        c.write_all(&req).await.unwrap();
        let mut reply = [0u8; 10];
        c.read_exact(&mut reply).await.unwrap();
        (c, reply[1])
    }

    type Tx = std::pin::Pin<Box<dyn Sink<Vec<u8>, Error = ()> + Send>>;
    type Rx = std::pin::Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

    /// An in-memory message channel as (sink, stream).
    fn pipe() -> (Tx, Rx) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
        let sink = futures_util::sink::unfold(tx, |tx, item: Vec<u8>| async move {
            tx.send(item).await.map_err(|_| ())?;
            Ok::<_, ()>(tx)
        });
        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        (Box::pin(sink), Box::pin(stream))
    }

    #[tokio::test]
    async fn round_trips_through_a_peer_and_refuses_without_one() {
        let echo = echo_server().await;
        let hub = Hub::new();
        let socks = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socks_addr = socks.local_addr().unwrap();
        tokio::spawn(serve_socks(socks, hub.clone(), peer_only()));

        // No peer: refused (never silently direct).
        let (_c, rep) = socks_connect(socks_addr, echo).await;
        assert_eq!(rep, 0x03);

        // Wire hub and peer together with in-memory channels.
        let (to_peer_tx, to_peer_rx) = pipe();
        let (to_hub_tx, to_hub_rx) = pipe();
        let hub2 = hub.clone();
        tokio::spawn(async move { hub2.run_peer(to_hub_rx, to_peer_tx).await });
        tokio::spawn(peer::serve_egress(
            to_peer_rx,
            to_hub_tx,
            |h: String, p| async move { TcpStream::connect((h.as_str(), p)).await },
        ));
        for _ in 0..100 {
            if hub.is_connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (mut c, rep) = socks_connect(socks_addr, echo).await;
        assert_eq!(rep, 0x00);
        c.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");
        assert_eq!(hub.stats().bytes_out.load(Ordering::SeqCst), 4);
        assert_eq!(hub.stats().bytes_in.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn forwards_to_the_target_the_peer_names() {
        let echo = echo_server().await;
        let hub = Hub::new();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_forward(listener, hub.clone(), "fs".into(), 0));
        // No peer: the connection is closed.
        let mut c = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 1];
        assert_eq!(c.read(&mut buf).await.unwrap(), 0);
        // The peer maps the target name to its own server.
        let (to_peer_tx, to_peer_rx) = pipe();
        let (to_hub_tx, to_hub_rx) = pipe();
        let hub2 = hub.clone();
        tokio::spawn(async move { hub2.run_peer(to_hub_rx, to_peer_tx).await });
        tokio::spawn(peer::serve_egress(
            to_peer_rx,
            to_hub_tx,
            move |h: String, _| async move {
                assert_eq!(h, "fs");
                TcpStream::connect(echo).await
            },
        ));
        for _ in 0..100 {
            if hub.is_connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"volume").await.unwrap();
        let mut got = [0u8; 6];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"volume");
    }
}
