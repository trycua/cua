// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A loopback portmapper for the Windows guest mount of Cua Volume.
//!
//! The Windows NFS client ("Client for NFS") has no port option: it asks
//! the portmapper on port 111 of the server where MOUNT and NFS listen. The
//! guest's volume listener (whose connections cua-spacesd relays to the
//! client over `/volume`) is on another port, so this answers those
//! questions on `127.0.0.1:111`, over TCP and UDP, for portmap version 2 and
//! rpcbind versions 3 and 4:
//!
//! - MOUNT (100005) and NFS (100003) version 3 over TCP: the listener.
//! - The same over UDP: a [`UdpBridge`] on the listener's port number,
//!   which carries each datagram to the listener as a TCP record (the host
//!   serves NFS over TCP only). The Windows redirector may use UDP when its
//!   transport setting is `TCP+UDP`.
//! - Anything else (NLM, other versions): not registered; the client mounts
//!   with `nolock`.
//!
//! It keeps the last calls it answered for diagnostics, serves nothing
//! else and binds loopback only.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, UdpSocket};

/// The portmapper's well-known port.
pub const PORTMAP_PORT: u16 = 111;

const PMAP_PROG: u32 = 100_000;
const NFS_PROG: u32 = 100_003;
const MOUNT_PROG: u32 = 100_005;
const IPPROTO_TCP: u32 = 6;
const IPPROTO_UDP: u32 = 17;

const CALL: u32 = 0;
const REPLY: u32 = 1;
const MSG_ACCEPTED: u32 = 0;
const SUCCESS: u32 = 0;
const PROG_UNAVAIL: u32 = 1;
const PROG_MISMATCH: u32 = 2;
const PROC_UNAVAIL: u32 = 3;
const GARBAGE_ARGS: u32 = 4;

/// Largest call accepted (portmap calls are a few dozen bytes).
const MAX_CALL: usize = 64 * 1024;

/// Where MOUNT and NFS listen (0: not registered on that transport).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ports {
    pub tcp: u16,
    pub udp: u16,
}

/// How many answered calls [`Portmap::calls`] keeps.
const LOG_LEN: usize = 32;

type CallLog = Arc<Mutex<VecDeque<String>>>;

fn note(log: &CallLog, transport: &str, what: String) {
    let mut l = log.lock().expect("portmap log");
    if l.len() == LOG_LEN {
        l.pop_front();
    }
    l.push_back(format!("{transport} {what}"));
}

/// A running portmapper; dropping it stops it.
pub struct Portmap {
    tasks: Vec<tokio::task::JoinHandle<()>>,
    addr: SocketAddr,
    log: CallLog,
}

impl std::fmt::Debug for Portmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Portmap").field("addr", &self.addr).finish()
    }
}

impl Drop for Portmap {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Portmap {
    /// Serves on `127.0.0.1:port` (TCP and UDP) and points MOUNT and NFS at
    /// `nfs`. `port` 0 picks one (tests).
    pub async fn start(port: u16, nfs: Ports) -> std::io::Result<Portmap> {
        let tcp = TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = tcp.local_addr()?;
        let udp = UdpSocket::bind(("127.0.0.1", addr.port())).await?;
        let log = CallLog::default();
        let l = log.clone();
        let tcp_task = tokio::spawn(async move {
            while let Ok((stream, _)) = tcp.accept().await {
                tokio::spawn(serve_tcp(stream, nfs, l.clone()));
            }
        });
        let l = log.clone();
        let udp_task = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_CALL];
            while let Ok((n, peer)) = udp.recv_from(&mut buf).await {
                if let Some((reply, what)) = answer_noted(&buf[..n], nfs) {
                    note(&l, "udp", what);
                    let _ = udp.send_to(&reply, peer).await;
                }
            }
        });
        Ok(Portmap {
            tasks: vec![tcp_task, udp_task],
            addr,
            log,
        })
    }

    /// Where it listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The last calls answered, oldest first (`udp GETPORT 100003 v3 udp -> 2049`).
    pub fn calls(&self) -> Vec<String> {
        self.log
            .lock()
            .expect("portmap log")
            .iter()
            .cloned()
            .collect()
    }
}

/// Reads one record-marked RPC message, or `None` at the end of the stream.
async fn read_record<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Option<Vec<u8>> {
    let mut msg = Vec::new();
    loop {
        let mut mark = [0u8; 4];
        r.read_exact(&mut mark).await.ok()?;
        let mark = u32::from_be_bytes(mark);
        let len = (mark & 0x7fff_ffff) as usize;
        if msg.len() + len > MAX_RECORD {
            return None;
        }
        let start = msg.len();
        msg.resize(start + len, 0);
        r.read_exact(&mut msg[start..]).await.ok()?;
        if mark & 0x8000_0000 != 0 {
            return Some(msg);
        }
    }
}

fn record(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + 4);
    out.extend_from_slice(&(0x8000_0000 | msg.len() as u32).to_be_bytes());
    out.extend_from_slice(msg);
    out
}

/// One TCP connection: record-marked calls, one reply each.
async fn serve_tcp(mut stream: tokio::net::TcpStream, nfs: Ports, log: CallLog) {
    loop {
        let Some(call) = read_record(&mut stream).await else {
            return;
        };
        if call.len() > MAX_CALL {
            return;
        }
        let Some((reply, what)) = answer_noted(&call, nfs) else {
            return;
        };
        note(&log, "tcp", what);
        if stream.write_all(&record(&reply)).await.is_err() {
            return;
        }
    }
}

/// Largest record the bridge carries (an NFS READ or WRITE of 32 KiB plus
/// headers, with room to spare).
const MAX_RECORD: usize = 1024 * 1024;

/// UDP to TCP for ONC RPC: each datagram is one call, sent as one record on
/// a TCP connection to the listener; each reply goes back to the datagram's
/// sender, matched by its transaction id. Dropping it stops it.
pub struct UdpBridge {
    task: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
    calls: Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for UdpBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdpBridge")
            .field("addr", &self.addr)
            .finish()
    }
}

impl Drop for UdpBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type Waiting = Arc<Mutex<HashMap<u32, SocketAddr>>>;

impl UdpBridge {
    /// Listens on UDP `127.0.0.1:port` (0 picks one) and carries calls to
    /// TCP `127.0.0.1:target`.
    pub async fn start(port: u16, target: u16) -> std::io::Result<UdpBridge> {
        let udp = Arc::new(UdpSocket::bind(("127.0.0.1", port)).await?);
        let addr = udp.local_addr()?;
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counted = calls.clone();
        let task = tokio::spawn(async move {
            let waiting = Waiting::default();
            let mut conn: Option<(tokio::net::tcp::OwnedWriteHalf, tokio::task::JoinHandle<()>)> =
                None;
            let mut buf = vec![0u8; 65_536];
            while let Ok((n, peer)) = udp.recv_from(&mut buf).await {
                let dgram = &buf[..n];
                let Some(xid) = dgram.get(..4) else { continue };
                let xid = u32::from_be_bytes([xid[0], xid[1], xid[2], xid[3]]);
                counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                waiting.lock().expect("waiting").insert(xid, peer);
                // Send on the open connection; on failure open a new one
                // and send once more (the client retransmits otherwise).
                for _ in 0..2 {
                    if conn.as_ref().is_none_or(|(_, reader)| reader.is_finished()) {
                        conn = match tokio::net::TcpStream::connect(("127.0.0.1", target)).await {
                            Ok(s) => {
                                let _ = s.set_nodelay(true);
                                let (mut read, write) = s.into_split();
                                let (udp, waiting) = (udp.clone(), waiting.clone());
                                let reader = tokio::spawn(async move {
                                    while let Some(reply) = read_record(&mut read).await {
                                        let Some(x) = reply.get(..4) else { continue };
                                        let x = u32::from_be_bytes([x[0], x[1], x[2], x[3]]);
                                        let to = waiting.lock().expect("waiting").remove(&x);
                                        if let Some(to) = to {
                                            let _ = udp.send_to(&reply, to).await;
                                        }
                                    }
                                });
                                Some((write, reader))
                            }
                            Err(_) => None,
                        };
                    }
                    let Some((write, _)) = conn.as_mut() else {
                        break;
                    };
                    if write.write_all(&record(dgram)).await.is_ok() {
                        break;
                    }
                    conn = None;
                }
            }
            if let Some((_, reader)) = conn {
                reader.abort();
            }
        });
        Ok(UdpBridge { task, addr, calls })
    }

    /// Where it listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Datagrams carried so far.
    pub fn calls(&self) -> u64 {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// XDR reader over one call.
struct Xdr<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Xdr<'_> {
    fn u32(&mut self) -> Option<u32> {
        let b = self.buf.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn opaque(&mut self) -> Option<&[u8]> {
        let len = self.u32()? as usize;
        let padded = len.checked_add(3)? & !3;
        let b = self.buf.get(self.pos..self.pos.checked_add(len)?)?;
        if self.pos + padded > self.buf.len() {
            return None;
        }
        self.pos += padded;
        Some(b)
    }
}

fn reply_head(xid: u32, accept: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    for v in [xid, REPLY, MSG_ACCEPTED, 0, 0, accept] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
    out.resize(out.len() + ((4 - s.len() % 4) % 4), 0);
}

/// The port `prog`/`vers` answers on over `prot` (6 TCP, 17 UDP), or 0.
fn port_of(prog: u32, vers: u32, prot: u32, nfs: Ports) -> u16 {
    let nfs_port = match prot {
        IPPROTO_TCP => nfs.tcp,
        IPPROTO_UDP => nfs.udp,
        _ => 0,
    };
    match (prog, vers) {
        (NFS_PROG, 3) | (MOUNT_PROG, 3) => nfs_port,
        (PMAP_PROG, 2..=4) if matches!(prot, IPPROTO_TCP | IPPROTO_UDP) => PORTMAP_PORT,
        _ => 0,
    }
}

/// The protocol an rpcbind netid names (IPv4 only).
fn prot_of(netid: &[u8]) -> u32 {
    match netid {
        b"tcp" => IPPROTO_TCP,
        b"udp" => IPPROTO_UDP,
        _ => 0,
    }
}

fn prot_name(prot: u32) -> String {
    match prot {
        IPPROTO_TCP => "tcp".into(),
        IPPROTO_UDP => "udp".into(),
        p => format!("prot {p}"),
    }
}

/// The reply to one call, or `None` when it is not an RPC call at all.
pub fn answer(call: &[u8], nfs: Ports) -> Option<Vec<u8>> {
    answer_noted(call, nfs).map(|(reply, _)| reply)
}

/// [`answer`], and what was asked and answered.
fn answer_noted(call: &[u8], nfs: Ports) -> Option<(Vec<u8>, String)> {
    let mut x = Xdr { buf: call, pos: 0 };
    let xid = x.u32()?;
    if x.u32()? != CALL || x.u32()? != 2 {
        return None;
    }
    let (prog, vers, proc_) = (x.u32()?, x.u32()?, x.u32()?);
    // Credentials and verifier: any flavour, ignored.
    for _ in 0..2 {
        x.u32()?;
        x.opaque()?;
    }
    if prog != PMAP_PROG {
        return Some((
            reply_head(xid, PROG_UNAVAIL),
            format!("program {prog} v{vers} -> unavailable"),
        ));
    }
    if !(2..=4).contains(&vers) {
        let mut out = reply_head(xid, PROG_MISMATCH);
        put_u32(&mut out, 2);
        put_u32(&mut out, 4);
        return Some((out, format!("portmap v{vers} -> mismatch")));
    }
    match (vers, proc_) {
        // NULL
        (_, 0) => Some((reply_head(xid, SUCCESS), format!("v{vers} NULL"))),
        // PMAPPROC_GETPORT(prog, vers, prot, port) -> port
        (2, 3) => {
            let args = (x.u32(), x.u32(), x.u32(), x.u32());
            let (Some(p), Some(v), Some(prot), Some(_)) = args else {
                return Some((reply_head(xid, GARBAGE_ARGS), "GETPORT garbage".into()));
            };
            let port = port_of(p, v, prot, nfs);
            let mut out = reply_head(xid, SUCCESS);
            put_u32(&mut out, u32::from(port));
            Some((
                out,
                format!("GETPORT {p} v{v} {} -> {port}", prot_name(prot)),
            ))
        }
        // PMAPPROC_DUMP -> the TCP mappings.
        (2, 4) => {
            let mut out = reply_head(xid, SUCCESS);
            for prot in [IPPROTO_TCP, IPPROTO_UDP] {
                for (p, v) in [(PMAP_PROG, 2), (MOUNT_PROG, 3), (NFS_PROG, 3)] {
                    let port = port_of(p, v, prot, nfs);
                    if port != 0 {
                        for w in [1, p, v, prot, u32::from(port)] {
                            put_u32(&mut out, w);
                        }
                    }
                }
            }
            put_u32(&mut out, 0);
            Some((out, "DUMP".into()))
        }
        // RPCBPROC_GETADDR(prog, vers, netid, addr, owner) -> universal address
        (3 | 4, 3) => {
            let parsed = (|| {
                let (p, v) = (x.u32()?, x.u32()?);
                let netid = x.opaque()?.to_vec();
                x.opaque()?;
                x.opaque()?;
                Some((p, v, netid))
            })();
            let Some((p, v, netid)) = parsed else {
                return Some((reply_head(xid, GARBAGE_ARGS), "GETADDR garbage".into()));
            };
            let port = port_of(p, v, prot_of(&netid), nfs);
            let what = format!(
                "rpcbind v{vers} GETADDR {p} v{v} {} -> {port}",
                String::from_utf8_lossy(&netid)
            );
            let mut out = reply_head(xid, SUCCESS);
            if port == 0 {
                put_string(&mut out, "");
            } else {
                put_string(
                    &mut out,
                    &format!("127.0.0.1.{}.{}", port >> 8, port & 0xff),
                );
            }
            Some((out, what))
        }
        _ => Some((
            reply_head(xid, PROC_UNAVAIL),
            format!("v{vers} proc {proc_} -> unavailable"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(vers: u32, proc_: u32, args: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        // xid, CALL, rpcvers 2, portmap, vers, proc, AUTH_UNIX cred, null verf
        for w in [7, CALL, 2, PMAP_PROG, vers, proc_, 1, 8, 0, 0, 0, 0] {
            put_u32(&mut c, w);
        }
        c.extend_from_slice(args);
        c
    }

    fn words(b: &[u8]) -> Vec<u32> {
        b.chunks(4)
            .map(|w| u32::from_be_bytes([w[0], w[1], w[2], w[3]]))
            .collect()
    }

    const NFS: Ports = Ports {
        tcp: 2049,
        udp: 2050,
    };
    const P1: Ports = Ports { tcp: 1, udp: 1 };

    fn getport(prog: u32, vers: u32, prot: u32) -> u32 {
        let mut a = Vec::new();
        for w in [prog, vers, prot, 0] {
            put_u32(&mut a, w);
        }
        let r = words(&answer(&call(2, 3, &a), NFS).unwrap());
        assert_eq!(&r[..6], &[7, REPLY, MSG_ACCEPTED, 0, 0, SUCCESS]);
        r[6]
    }

    #[test]
    fn mount_and_nfs_v3_over_tcp_point_at_the_listener() {
        assert_eq!(getport(MOUNT_PROG, 3, IPPROTO_TCP), 2049);
        assert_eq!(getport(NFS_PROG, 3, IPPROTO_TCP), 2049);
        assert_eq!(getport(MOUNT_PROG, 3, IPPROTO_UDP), 2050);
        assert_eq!(getport(NFS_PROG, 3, IPPROTO_UDP), 2050);
        // NLM, MOUNT v1 and other transports are not registered.
        assert_eq!(getport(NFS_PROG, 3, 99), 0);
        assert_eq!(getport(100_021, 4, IPPROTO_TCP), 0);
        assert_eq!(getport(MOUNT_PROG, 1, IPPROTO_TCP), 0);
    }

    #[test]
    fn rpcbind_getaddr_answers_a_universal_address() {
        let mut a = Vec::new();
        put_u32(&mut a, NFS_PROG);
        put_u32(&mut a, 3);
        put_string(&mut a, "tcp");
        put_string(&mut a, "");
        put_string(&mut a, "");
        let r = answer(&call(4, 3, &a), NFS).unwrap();
        assert_eq!(&words(&r)[..6], &[7, REPLY, MSG_ACCEPTED, 0, 0, SUCCESS]);
        let mut x = Xdr { buf: &r, pos: 24 };
        assert_eq!(x.opaque().unwrap(), b"127.0.0.1.8.1");
    }

    #[test]
    fn other_programs_versions_and_garbage() {
        let mut c = call(2, 3, &[]);
        c[12..16].copy_from_slice(&NFS_PROG.to_be_bytes());
        assert_eq!(words(&answer(&c, P1).unwrap())[5], PROG_UNAVAIL);
        assert_eq!(
            words(&answer(&call(5, 0, &[]), P1).unwrap())[5..],
            [PROG_MISMATCH, 2, 4]
        );
        assert_eq!(
            words(&answer(&call(2, 3, &[]), P1).unwrap())[5],
            GARBAGE_ARGS
        );
        assert_eq!(
            words(&answer(&call(2, 9, &[]), P1).unwrap())[5],
            PROC_UNAVAIL
        );
        assert_eq!(words(&answer(&call(2, 0, &[]), P1).unwrap())[5], SUCCESS);
        assert!(answer(&[0, 0, 0, 1], P1).is_none());
        // A reply is not a call.
        let mut r = call(2, 0, &[]);
        r[4..8].copy_from_slice(&REPLY.to_be_bytes());
        assert!(answer(&r, P1).is_none());
    }

    #[tokio::test]
    async fn serves_over_tcp_and_udp() {
        let pm = Portmap::start(0, Ports { tcp: 4321, udp: 0 })
            .await
            .unwrap();
        let mut a = Vec::new();
        for w in [NFS_PROG, 3, IPPROTO_TCP, 0] {
            put_u32(&mut a, w);
        }
        let c = call(2, 3, &a);
        // UDP
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.send_to(&c, pm.addr()).await.unwrap();
        let mut buf = [0u8; 256];
        let n = s.recv(&mut buf).await.unwrap();
        assert_eq!(words(&buf[..n])[6], 4321);
        // TCP, the call split over two fragments.
        let mut t = tokio::net::TcpStream::connect(pm.addr()).await.unwrap();
        let (a1, a2) = c.split_at(10);
        let mut out = Vec::new();
        out.extend_from_slice(&(a1.len() as u32).to_be_bytes());
        out.extend_from_slice(a1);
        out.extend_from_slice(&(0x8000_0000 | a2.len() as u32).to_be_bytes());
        out.extend_from_slice(a2);
        t.write_all(&out).await.unwrap();
        let mut mark = [0u8; 4];
        t.read_exact(&mut mark).await.unwrap();
        let len = (u32::from_be_bytes(mark) & 0x7fff_ffff) as usize;
        let mut r = vec![0u8; len];
        t.read_exact(&mut r).await.unwrap();
        assert_eq!(words(&r)[6], 4321);
        let calls = pm.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls[0].starts_with("udp GETPORT 100003 v3 tcp -> 4321"),
            "{calls:?}"
        );
    }

    /// A TCP RPC server that answers each record with its xid and length.
    async fn echo_server() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    while let Some(msg) = read_record(&mut s).await {
                        let mut reply = msg[..4].to_vec();
                        put_u32(&mut reply, msg.len() as u32);
                        s.write_all(&record(&reply)).await.unwrap();
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn the_udp_bridge_matches_replies_to_senders() {
        let target = echo_server().await;
        let bridge = UdpBridge::start(0, target).await.unwrap();
        let (a, b) = (
            UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        );
        let mut big = vec![0u8; 9_000];
        big[..4].copy_from_slice(&11u32.to_be_bytes());
        a.send_to(&big, bridge.addr()).await.unwrap();
        b.send_to(&[0, 0, 0, 22, 9, 9], bridge.addr())
            .await
            .unwrap();
        let mut buf = [0u8; 64];
        let n = a.recv(&mut buf).await.unwrap();
        assert_eq!(words(&buf[..n]), [11, 9_000]);
        let n = b.recv(&mut buf).await.unwrap();
        assert_eq!(words(&buf[..n]), [22, 6]);
        assert_eq!(bridge.calls(), 2);
    }
}
