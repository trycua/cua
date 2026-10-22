// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Virtual UDP sockets on the node's mesh address.
//!
//! Packets the tunnel decrypts land here by destination port; packets a
//! socket sends become IPv6/UDP packets the engine encrypts to the peer
//! that owns the destination address. A [`VSocket`] is also a quinn
//! [`AsyncUdpSocket`], so QUIC runs over the mesh unchanged.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use bytes::Bytes;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::sync::mpsc;

use crate::packet;

/// Where the engine sends what a socket writes.
pub(crate) trait Outbound: Send + Sync + 'static {
    /// Encrypts and sends one IP packet to the peer owning `dst`.
    fn send_ip(&self, dst: Ipv6Addr, packet: &[u8]) -> io::Result<()>;
    /// Ready to send to `dst` (the underlying path's socket is writable).
    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

const QUEUE: usize = 4096;

type Inbox = mpsc::Sender<(SocketAddrV6, Bytes)>;

/// The node's virtual network: its address and bound ports.
pub(crate) struct VNet {
    addr: Ipv6Addr,
    ports: Mutex<HashMap<u16, Inbox>>,
    out: Mutex<Option<Weak<dyn Outbound>>>,
}

impl fmt::Debug for VNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VNet").field("addr", &self.addr).finish()
    }
}

impl VNet {
    pub(crate) fn new(addr: Ipv6Addr) -> Arc<Self> {
        Arc::new(Self {
            addr,
            ports: Mutex::new(HashMap::new()),
            out: Mutex::new(None),
        })
    }

    pub(crate) fn set_outbound(&self, out: Weak<dyn Outbound>) {
        *self.out.lock().expect("out") = Some(out);
    }

    fn outbound(&self) -> io::Result<Arc<dyn Outbound>> {
        self.out
            .lock()
            .expect("out")
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "mesh stopped"))
    }

    /// Delivers a decrypted packet (already checked to come from its
    /// peer's address). Drops it when nothing listens or the queue is full.
    pub(crate) fn deliver(&self, p: &[u8]) -> bool {
        let Some(u) = packet::parse_udp(p) else {
            return false;
        };
        if *u.dst.ip() != self.addr {
            return false;
        }
        let tx = self
            .ports
            .lock()
            .expect("ports")
            .get(&u.dst.port())
            .cloned();
        match tx {
            Some(tx) => tx
                .try_send((u.src, Bytes::copy_from_slice(u.payload)))
                .is_ok(),
            None => false,
        }
    }

    /// Binds `port` (0: an ephemeral one).
    pub(crate) fn bind(self: &Arc<Self>, port: u16) -> io::Result<Arc<VSocket>> {
        let mut ports = self.ports.lock().expect("ports");
        let port = if port == 0 {
            let mut chosen = None;
            for _ in 0..64 {
                let p = rand::random_range(20_000u16..60_000);
                if !ports.contains_key(&p) {
                    chosen = Some(p);
                    break;
                }
            }
            chosen.ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no free mesh port"))?
        } else if ports.contains_key(&port) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("mesh port {port} in use"),
            ));
        } else {
            port
        };
        let (tx, rx) = mpsc::channel(QUEUE);
        ports.insert(port, tx);
        Ok(Arc::new(VSocket {
            local: SocketAddrV6::new(self.addr, port, 0, 0),
            rx: Mutex::new(rx),
            net: Arc::downgrade(self),
        }))
    }

    fn unbind(&self, port: u16) {
        self.ports.lock().expect("ports").remove(&port);
    }
}

/// A UDP socket on the mesh.
pub struct VSocket {
    local: SocketAddrV6,
    rx: Mutex<mpsc::Receiver<(SocketAddrV6, Bytes)>>,
    net: Weak<VNet>,
}

impl fmt::Debug for VSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VSocket")
            .field("local", &self.local)
            .finish()
    }
}

impl Drop for VSocket {
    fn drop(&mut self) {
        if let Some(net) = self.net.upgrade() {
            net.unbind(self.local.port());
        }
    }
}

impl VSocket {
    /// The bound mesh address.
    pub fn local(&self) -> SocketAddrV6 {
        self.local
    }

    /// Sends one datagram to `dst` on the mesh.
    pub fn send_to(&self, payload: &[u8], dst: SocketAddrV6) -> io::Result<()> {
        let p = packet::build_udp(self.local, dst, payload).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "datagram larger than the mesh MTU",
            )
        })?;
        let net = self
            .net
            .upgrade()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "mesh stopped"))?;
        net.outbound()?.send_ip(*dst.ip(), &p)
    }

    /// Receives one datagram.
    pub async fn recv_from(&self) -> io::Result<(Bytes, SocketAddrV6)> {
        std::future::poll_fn(|cx| {
            let mut rx = self.rx.lock().expect("rx");
            match rx.poll_recv(cx) {
                Poll::Ready(Some((src, b))) => Poll::Ready(Ok((b, src))),
                Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "mesh stopped",
                ))),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }
}

#[derive(Debug)]
struct Poller {
    net: Weak<VNet>,
}

impl UdpPoller for Poller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match self.net.upgrade().map(|n| n.outbound()) {
            Some(Ok(out)) => out.poll_send_ready(cx),
            Some(Err(e)) => Poll::Ready(Err(e)),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "mesh stopped",
            ))),
        }
    }
}

impl AsyncUdpSocket for VSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Poller {
            net: self.net.clone(),
        })
    }

    fn try_send(&self, t: &Transmit) -> io::Result<()> {
        let SocketAddr::V6(dst) = t.destination else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the mesh is IPv6 only",
            ));
        };
        match t.segment_size {
            Some(seg) if seg < t.contents.len() => {
                for chunk in t.contents.chunks(seg) {
                    self.send_to(chunk, dst)?;
                }
                Ok(())
            }
            _ => self.send_to(t.contents, dst),
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut rx = self.rx.lock().expect("rx");
        let mut n = 0;
        while n < bufs.len() && n < meta.len() {
            let next = if n == 0 {
                match rx.poll_recv(cx) {
                    Poll::Ready(Some(v)) => v,
                    Poll::Ready(None) => {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::NotConnected,
                            "mesh stopped",
                        )));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            } else {
                match rx.try_recv() {
                    Ok(v) => v,
                    Err(_) => break,
                }
            };
            let (src, payload) = next;
            let len = payload.len().min(bufs[n].len());
            bufs[n][..len].copy_from_slice(&payload[..len]);
            meta[n] = RecvMeta {
                addr: SocketAddr::V6(src),
                len,
                stride: len,
                ecn: None,
                dst_ip: Some(std::net::IpAddr::V6(*self.local.ip())),
            };
            n += 1;
        }
        Poll::Ready(Ok(n))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V6(self.local))
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Loop(Arc<VNet>);
    impl Outbound for Loop {
        fn send_ip(&self, _dst: Ipv6Addr, packet: &[u8]) -> io::Result<()> {
            self.0.deliver(packet);
            Ok(())
        }
        fn poll_send_ready(&self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn sockets_exchange_datagrams() {
        let addr: Ipv6Addr = "fd5c:a000::1".parse().unwrap();
        let net = VNet::new(addr);
        let out: Arc<dyn Outbound> = Arc::new(Loop(net.clone()));
        net.set_outbound(Arc::downgrade(&out));
        let a = net.bind(0).unwrap();
        let b = net.bind(7443).unwrap();
        assert!(net.bind(7443).is_err());
        a.send_to(b"ping", b.local()).unwrap();
        let (got, from) = b.recv_from().await.unwrap();
        assert_eq!(&got[..], b"ping");
        assert_eq!(from, a.local());
        drop(b);
        assert!(net.bind(7443).is_ok());
    }
}
