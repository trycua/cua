// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Share slots: public TCP shares served straight from a Space's own cloud
//! machine (CUA-2602), with no relay and no extra server.
//!
//! The computer publishes a small range of ports from the Space
//! (`CUA_SHARE_SLOTS`, e.g. `26500-26515`; the cloud's firewall keeps them
//! closed until a share opens one). [`PublishShareSlot`] makes this spacesd
//! listen on one slot and forward each connection to one port of the
//! Space's *own loopback*: never another address, so a slot reaches the
//! shared service and nothing else in the Space, the computer or the cloud.
//!
//! Per connection: the allowlist (on the client's real address: the
//! firewall passes it unchanged), a per-address connection rate, a cap of
//! open connections, and for Minecraft Java the handshake's host. A slot
//! ends at its TTL (24 h at most), on [`UnpublishShareSlot`], or when this
//! spacesd stops.
//!
//! [`PublishShareSlot`]: cua_proto::env::v1::tunnel_service_server::TunnelService::publish_share_slot
//! [`UnpublishShareSlot`]: cua_proto::env::v1::tunnel_service_server::TunnelService::unpublish_share_slot

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

/// Environment variable naming the slot range (`26500-26515`).
pub const SLOTS_ENV: &str = "CUA_SHARE_SLOTS";
/// The longest a slot serves.
pub const MAX_TTL: Duration = Duration::from_secs(24 * 3600);
/// Ports a slot never reaches.
pub const REFUSED: &[u16] = &[
    22, 23, 2375, 2376, 3306, 3389, 5432, 5900, 5901, 6379, 6443, 10250, 25575, 27017,
];
/// Open connections per slot.
pub const MAX_CONNS: usize = 64;
/// New connections per second from one address (burst 10).
const CONN_RPS: f64 = 2.0;
const CONN_BURST: f64 = 10.0;

/// The slot range of this computer, from [`SLOTS_ENV`].
pub fn slot_range() -> Option<(u16, u16)> {
    let v = std::env::var(SLOTS_ENV).ok()?;
    let (a, b) = v.trim().split_once('-')?;
    let (a, b): (u16, u16) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a > 0 && b >= a && b - a < 64).then_some((a, b))
}

/// What a publish asked for, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSpec {
    /// The slot.
    pub slot: u16,
    /// The Space port (on 127.0.0.1).
    pub port: u16,
    /// Allowed CIDRs (normalized); empty: anyone.
    pub allow: Vec<(IpAddr, u8)>,
    /// Hosts the Minecraft handshake must name; empty: no check.
    pub expect_hosts: Vec<String>,
    /// How long.
    pub ttl: Duration,
    /// The most bytes it may move (both ways); 0: no cap.
    pub max_bytes: u64,
}

/// Checks a publish against the computer's slot range and the rules.
pub fn check(
    range: Option<(u16, u16)>,
    slot: u32,
    port: u32,
    allow: &[String],
    expect_hosts: &[String],
    ttl: Option<Duration>,
    max_bytes: u64,
) -> Result<SlotSpec, String> {
    let Some((first, last)) = range else {
        return Err("this computer publishes no share slots".into());
    };
    let slot = u16::try_from(slot).map_err(|_| "slot out of range".to_string())?;
    if !(first..=last).contains(&slot) {
        return Err(format!(
            "slot {slot} is not one of this computer's ({first}-{last})"
        ));
    }
    let port = u16::try_from(port)
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| "port must be 1-65535".to_string())?;
    if REFUSED.contains(&port) {
        return Err(format!(
            "port {port} is an admin or remote-access port and cannot be shared publicly"
        ));
    }
    if (first..=last).contains(&port) {
        return Err("a slot cannot forward to another slot".into());
    }
    if allow.len() > 32 {
        return Err("at most 32 allowlist entries".into());
    }
    let allow = allow
        .iter()
        .map(|c| parse_cidr(c).ok_or_else(|| format!("{c:?} is not an address or CIDR")))
        .collect::<Result<Vec<_>, _>>()?;
    let ttl = ttl
        .unwrap_or(MAX_TTL)
        .min(MAX_TTL)
        .max(Duration::from_secs(60));
    Ok(SlotSpec {
        slot,
        port,
        allow,
        expect_hosts: expect_hosts
            .iter()
            .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect(),
        ttl,
        max_bytes,
    })
}

fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let s = s.trim();
    let (ip, len) = match s.split_once('/') {
        Some((ip, len)) => (ip, Some(len)),
        None => (s, None),
    };
    let ip: IpAddr = ip.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len: u8 = match len {
        Some(l) => l.parse().ok().filter(|l| *l <= max)?,
        None => max,
    };
    Some((ip, len))
}

/// Whether `ip` is in one of `allow` (empty: anyone).
pub fn allowed(allow: &[(IpAddr, u8)], ip: IpAddr) -> bool {
    if allow.is_empty() {
        return true;
    }
    allow.iter().any(|(net, len)| match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) => {
            let mask = if *len == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(*len))
            };
            u32::from(*n) & mask == u32::from(i) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(i)) => {
            let mask = if *len == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(*len))
            };
            u128::from(*n) & mask == u128::from(i) & mask
        }
        _ => false,
    })
}

/// The host a Minecraft Java handshake names (see cua-relay's
/// `mc_handshake`, the same rules): `Some(Ok(host))`, `Some(Err(()))` when
/// it is not a handshake, `None` when more bytes are needed.
pub fn mc_host(b: &[u8]) -> Option<Result<String, ()>> {
    fn varint(b: &[u8], at: &mut usize) -> Option<Result<u32, ()>> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let byte = *b.get(*at)?;
            *at += 1;
            v |= u32::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                return Some(Ok(v));
            }
        }
        Some(Err(()))
    }
    if b.first() == Some(&0xFE) {
        return Some(Err(()));
    }
    let more = || if b.len() >= 512 { Some(Err(())) } else { None };
    let mut at = 0;
    let len = match varint(b, &mut at) {
        None => return more(),
        Some(Err(())) => return Some(Err(())),
        Some(Ok(l)) => l as usize,
    };
    if len == 0 || len > 512 {
        return Some(Err(()));
    }
    let start = at;
    match varint(b, &mut at) {
        None => return more(),
        Some(Ok(0)) => {}
        _ => return Some(Err(())),
    }
    match varint(b, &mut at) {
        None => return more(),
        Some(Err(())) => return Some(Err(())),
        Some(Ok(_)) => {}
    }
    let slen = match varint(b, &mut at) {
        None => return more(),
        Some(Err(())) => return Some(Err(())),
        Some(Ok(l)) => l as usize,
    };
    if slen == 0 || at + slen > start + len {
        return Some(Err(()));
    }
    if b.len() < at + slen {
        return more();
    }
    let Ok(raw) = std::str::from_utf8(&b[at..at + slen]) else {
        return Some(Err(()));
    };
    let host = raw
        .split('\0')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Some(Err(()));
    }
    Some(Ok(host))
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// One served slot.
struct Served {
    cut: CancellationToken,
    expires: SystemTime,
    port: u16,
    max_bytes: u64,
    moved: Arc<AtomicU64>,
    open: Arc<std::sync::atomic::AtomicUsize>,
}

/// A served slot, as [`ShareSlots::stats`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotStats {
    /// The slot.
    pub slot: u16,
    /// The Space port.
    pub port: u16,
    /// Bytes moved since it was published.
    pub bytes: u64,
    /// Its data cap (0: none).
    pub max_bytes: u64,
    /// Open connections.
    pub connections: u32,
    /// When it stops.
    pub expires: SystemTime,
}

/// The share slots this spacesd serves.
#[derive(Default, Clone)]
pub struct ShareSlots {
    served: Arc<Mutex<HashMap<u16, Served>>>,
    rate: Arc<Mutex<HashMap<IpAddr, Bucket>>>,
}

impl ShareSlots {
    /// Serves `spec`, replacing what the slot served before. The listener
    /// binds every address of the computer-facing interface (the computer
    /// publishes the slot from the Space); connections go to 127.0.0.1 only.
    pub async fn publish(&self, spec: SlotSpec) -> std::io::Result<SystemTime> {
        self.unpublish(spec.slot);
        let listener =
            tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], spec.slot))).await?;
        let cut = CancellationToken::new();
        let expires = SystemTime::now() + spec.ttl;
        let moved = Arc::new(AtomicU64::new(0));
        let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.served.lock().expect("slots").insert(
            spec.slot,
            Served {
                cut: cut.clone(),
                expires,
                port: spec.port,
                max_bytes: spec.max_bytes,
                moved: moved.clone(),
                open: open.clone(),
            },
        );
        let me = self.clone();
        let spec = Arc::new(spec);
        tokio::spawn(async move {
            let until = tokio::time::sleep(spec.ttl);
            tokio::pin!(until);
            loop {
                let (stream, peer) = tokio::select! {
                    _ = cut.cancelled() => break,
                    _ = &mut until => break,
                    r = listener.accept() => match r {
                        Ok(v) => v,
                        Err(_) => continue,
                    },
                };
                if !allowed(&spec.allow, peer.ip()) || !me.admit(peer.ip()) {
                    continue;
                }
                // Over its data cap: no new connections.
                if spec.max_bytes > 0 && moved.load(Ordering::Relaxed) >= spec.max_bytes {
                    continue;
                }
                if open.load(std::sync::atomic::Ordering::SeqCst) >= MAX_CONNS {
                    continue;
                }
                open.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (spec, cut, open, moved) =
                    (spec.clone(), cut.clone(), open.clone(), moved.clone());
                tokio::spawn(async move {
                    let _ = serve(stream, &spec, cut, moved).await;
                    open.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            }
            me.served.lock().expect("slots").remove(&spec.slot);
            cut.cancel();
        });
        Ok(expires)
    }

    /// Stops serving `slot`; its connections close.
    pub fn unpublish(&self, slot: u16) {
        if let Some(s) = self.served.lock().expect("slots").remove(&slot) {
            s.cut.cancel();
        }
    }

    /// The slots served now, with when each stops.
    pub fn served(&self) -> Vec<(u16, SystemTime)> {
        self.served
            .lock()
            .expect("slots")
            .iter()
            .map(|(k, v)| (*k, v.expires))
            .collect()
    }

    /// Every served slot with what it moved.
    pub fn stats(&self) -> Vec<SlotStats> {
        let mut v: Vec<SlotStats> = self
            .served
            .lock()
            .expect("slots")
            .iter()
            .map(|(k, s)| SlotStats {
                slot: *k,
                port: s.port,
                bytes: s.moved.load(Ordering::Relaxed),
                max_bytes: s.max_bytes,
                connections: s.open.load(Ordering::Relaxed) as u32,
                expires: s.expires,
            })
            .collect();
        v.sort_by_key(|s| s.slot);
        v
    }

    fn admit(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut g = self.rate.lock().expect("rate");
        if g.len() > 50_000 {
            g.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(300));
        }
        let b = g.entry(ip).or_insert(Bucket {
            tokens: CONN_BURST,
            last: now,
        });
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * CONN_RPS).min(CONN_BURST);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

async fn serve(
    mut client: tokio::net::TcpStream,
    spec: &SlotSpec,
    cut: CancellationToken,
    moved: Arc<AtomicU64>,
) -> std::io::Result<()> {
    let mut first = Vec::new();
    if !spec.expect_hosts.is_empty() {
        let check = async {
            let mut buf = [0u8; 256];
            loop {
                if let Some(r) = mc_host(&first) {
                    return r.ok();
                }
                let n = client.read(&mut buf).await.ok()?;
                if n == 0 {
                    return None;
                }
                first.extend_from_slice(&buf[..n]);
            }
        };
        let host = tokio::time::timeout(Duration::from_secs(5), check)
            .await
            .ok()
            .flatten();
        if !host.is_some_and(|h| spec.expect_hosts.contains(&h)) {
            return Ok(());
        }
    }
    // The Space's own loopback, nothing else.
    let mut upstream = tokio::net::TcpStream::connect(("127.0.0.1", spec.port)).await?;
    let _ = upstream.set_nodelay(true);
    if !first.is_empty() {
        moved.fetch_add(first.len() as u64, Ordering::Relaxed);
        upstream.write_all(&first).await?;
    }
    let (mut cr, mut cw) = client.split();
    let (mut ur, mut uw) = upstream.split();
    let max = spec.max_bytes;
    let over = CancellationToken::new();
    tokio::select! {
        _ = cut.cancelled() => {}
        _ = over.cancelled() => {}
        _ = async {
            tokio::join!(
                pump(&mut cr, &mut uw, &moved, max, &over),
                pump(&mut ur, &mut cw, &moved, max, &over),
            )
        } => {}
    }
    Ok(())
}

/// Copies `r` to `w`, counting into `moved`; at the cap (`max` > 0) it
/// cancels `over`, which ends this connection (the listener refuses new
/// ones while the slot is over its cap).
async fn pump<R, W>(r: &mut R, w: &mut W, moved: &AtomicU64, max: u64, over: &CancellationToken)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let total = moved.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if w.write_all(&buf[..n]).await.is_err() {
            break;
        }
        if max > 0 && total >= max {
            over.cancel();
            break;
        }
    }
    let _ = w.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handshake(host: &str) -> Vec<u8> {
        let mut body = vec![0x00, 0xff, 0x05, host.len() as u8];
        body.extend_from_slice(host.as_bytes());
        body.extend_from_slice(&[0x63, 0xdd, 0x01]);
        let mut out = vec![body.len() as u8];
        out.extend(body);
        out
    }

    #[test]
    fn a_publish_names_a_slot_and_a_space_port_only() {
        let r = Some((26500, 26515));
        assert!(check(None, 26500, 25565, &[], &[], None, 0).is_err());
        assert!(check(r, 26499, 25565, &[], &[], None, 0).is_err());
        assert!(check(r, 26516, 25565, &[], &[], None, 0).is_err());
        for admin in [22, 3389, 5900, 25575, 5432] {
            assert!(
                check(r, 26500, admin, &[], &[], None, 0).is_err(),
                "{admin}"
            );
        }
        assert!(
            check(r, 26500, 26501, &[], &[], None, 0).is_err(),
            "slot to slot"
        );
        assert!(check(r, 26500, 0, &[], &[], None, 0).is_err());
        let s = check(
            r,
            26500,
            25565,
            &["203.0.113.0/24".into()],
            &["MC-1.example.".into()],
            Some(Duration::from_secs(48 * 3600)),
            1 << 20,
        )
        .unwrap();
        assert_eq!(s.max_bytes, 1 << 20);
        assert_eq!(s.ttl, MAX_TTL);
        assert_eq!(s.expect_hosts, vec!["mc-1.example"]);
        assert!(allowed(&s.allow, "203.0.113.4".parse().unwrap()));
        assert!(!allowed(&s.allow, "198.51.100.4".parse().unwrap()));
        assert!(check(r, 26500, 25565, &["nope".into()], &[], None, 0).is_err());
    }

    #[test]
    fn the_minecraft_host_is_read_from_the_handshake() {
        let h = handshake("mc-1.example");
        assert_eq!(mc_host(&h), Some(Ok("mc-1.example".into())));
        assert_eq!(mc_host(&h[..3]), None);
        assert_eq!(mc_host(b"GET / HTTP/1.1"), Some(Err(())));
        assert_eq!(mc_host(&[0xFE, 0x01]), Some(Err(())));
    }

    async fn echo() -> u16 {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        port
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn a_slot_forwards_to_the_space_port_and_stops_when_unpublished() {
        let target = echo().await;
        let slot = free_port();
        let slots = ShareSlots::default();
        slots
            .publish(SlotSpec {
                slot,
                port: target,
                allow: vec![],
                expect_hosts: vec![],
                ttl: Duration::from_secs(60),
                max_bytes: 0,
            })
            .await
            .unwrap();
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", slot))
            .await
            .unwrap();
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"ping");
        assert_eq!(slots.served().len(), 1);
        slots.unpublish(slot);
        let mut one = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut one))
            .await
            .expect("closed in time")
            .unwrap_or(0);
        assert_eq!(n, 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(slots.served().is_empty());
    }

    #[tokio::test]
    async fn a_minecraft_slot_drops_the_wrong_host_and_passes_the_right_one() {
        let target = echo().await;
        let slot = free_port();
        let slots = ShareSlots::default();
        slots
            .publish(SlotSpec {
                slot,
                port: target,
                allow: vec![],
                expect_hosts: vec!["mc-1.example".into()],
                ttl: Duration::from_secs(60),
                max_bytes: 0,
            })
            .await
            .unwrap();
        let mut wrong = tokio::net::TcpStream::connect(("127.0.0.1", slot))
            .await
            .unwrap();
        wrong
            .write_all(&handshake("scanner.example"))
            .await
            .unwrap();
        let mut b = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(10), wrong.read(&mut b))
            .await
            .expect("dropped in time")
            .unwrap_or(0);
        assert_eq!(n, 0);
        let mut right = tokio::net::TcpStream::connect(("127.0.0.1", slot))
            .await
            .unwrap();
        let hs = handshake("mc-1.example");
        right.write_all(&hs).await.unwrap();
        let mut got = vec![0u8; hs.len()];
        tokio::time::timeout(Duration::from_secs(10), right.read_exact(&mut got))
            .await
            .expect("echo in time")
            .unwrap();
        assert_eq!(got, hs);
    }

    #[tokio::test]
    async fn an_allowlisted_slot_refuses_other_addresses() {
        let target = echo().await;
        let slot = free_port();
        let slots = ShareSlots::default();
        slots
            .publish(SlotSpec {
                slot,
                port: target,
                allow: vec![("203.0.113.0".parse().unwrap(), 24)],
                expect_hosts: vec![],
                ttl: Duration::from_secs(60),
                max_bytes: 0,
            })
            .await
            .unwrap();
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", slot))
            .await
            .unwrap();
        let _ = c.write_all(b"x").await;
        let mut b = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut b))
            .await
            .expect("dropped in time")
            .unwrap_or(0);
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn a_slot_reports_what_it_moved_and_stops_carrying_at_its_cap() {
        let target = echo().await;
        let slot = free_port();
        let slots = ShareSlots::default();
        slots
            .publish(SlotSpec {
                slot,
                port: target,
                allow: vec![],
                expect_hosts: vec![],
                ttl: Duration::from_secs(60),
                max_bytes: 8192,
            })
            .await
            .unwrap();
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", slot))
            .await
            .unwrap();
        c.write_all(&[1u8; 1024]).await.unwrap();
        let mut got = vec![0u8; 1024];
        c.read_exact(&mut got).await.unwrap();
        let st = slots.stats();
        assert_eq!(st.len(), 1);
        assert_eq!(
            (st[0].slot, st[0].port, st[0].max_bytes),
            (slot, target, 8192)
        );
        assert!(st[0].bytes >= 2048, "{st:?}");
        // Past the cap the connection ends...
        let _ = c.write_all(&[2u8; 16 * 1024]).await;
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut rest))
            .await
            .expect("closed in time");
        assert!(rest.len() < 16 * 1024);
        // ...no new one is carried, and the slot still reports its bytes.
        let mut d = tokio::net::TcpStream::connect(("127.0.0.1", slot))
            .await
            .unwrap();
        let _ = d.write_all(b"ping").await;
        let mut b = [0u8; 4];
        let n = tokio::time::timeout(Duration::from_secs(5), d.read(&mut b))
            .await
            .expect("refused in time")
            .unwrap_or(0);
        assert_eq!(n, 0);
        assert!(slots.stats()[0].bytes >= 8192);
    }
}
