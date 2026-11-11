// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A TCP proxy that makes one path bad, in user space.
//!
//! It touches nothing but the connections routed through it: no firewall,
//! no interface, no sysctl, no root, nothing about the machine's real
//! network (so a hotspot is never affected, and there is nothing to clean up
//! but the proxy itself). A node that dials the proxy's port instead of its
//! peer sees whatever the [`Shape`] says:
//!
//! - latency and jitter, a bandwidth cap;
//! - loss, modeled the way TCP experiences it: a lost packet costs a
//!   retransmission timeout (doubling while it keeps being lost), so 20 %
//!   loss shows up as stalls and a collapsed rate, not as missing bytes;
//! - `hold`: the path is blackholed, bytes stop and resume;
//! - `sever`: every connection is reset and new ones are refused;
//! - flapping: reset every N to M seconds for a short while.
//!
//! Each direction has its own shape (asymmetric loss).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;

/// What one direction of the path does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub latency_ms: u64,
    pub jitter_ms: u64,
    /// Bytes per second (0: unlimited).
    pub bandwidth: u64,
    /// Share of packets lost, 0 to 1.
    pub loss: f64,
    /// Bytes stop until cleared.
    pub hold: bool,
    /// The retransmission timeout a lost packet costs, at least (0: 200 ms,
    /// what TCP stacks use).
    pub rto_ms: u64,
}

/// Both directions: `up` is dialer to target, `down` is target to dialer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub up: Shape,
    pub down: Shape,
}

impl Path {
    pub fn both(s: Shape) -> Path {
        Path {
            up: s.clone(),
            down: s,
        }
    }
}

#[derive(Debug, Default)]
pub struct Stats {
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
    pub connections: AtomicU64,
    pub resets: AtomicU64,
    pub refused: AtomicU64,
}

struct Shared {
    /// When each direction's link is free again (all connections share its
    /// bandwidth and its queue, like a real link).
    free: Mutex<(Instant, Instant)>,
    path: Mutex<Path>,
    sever: AtomicBool,
    /// Bumped to reset every live connection.
    epoch: AtomicU64,
    stats: Stats,
    stop: AtomicBool,
}

/// A running proxy.
pub struct Proxy {
    shared: Arc<Shared>,
    pub listen: SocketAddr,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

const MSS: usize = 1448;

impl Proxy {
    /// Listens on `listen` (port 0 picks one) and forwards to `target`.
    pub async fn start(listen: SocketAddr, target: SocketAddr) -> std::io::Result<Proxy> {
        let l = TcpListener::bind(listen).await?;
        let listen = l.local_addr()?;
        let shared = Arc::new(Shared {
            free: Mutex::new((Instant::now(), Instant::now())),
            path: Mutex::new(Path::default()),
            sever: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            stats: Stats::default(),
            stop: AtomicBool::new(false),
        });
        let sh = shared.clone();
        let accept = tokio::spawn(async move {
            loop {
                let Ok((client, _)) = l.accept().await else {
                    return;
                };
                if sh.stop.load(Ordering::SeqCst) {
                    return;
                }
                if sh.sever.load(Ordering::SeqCst) {
                    sh.stats.refused.fetch_add(1, Ordering::Relaxed);
                    drop(client);
                    continue;
                }
                let sh2 = sh.clone();
                tokio::spawn(async move {
                    let _ = connection(sh2, client, target).await;
                });
            }
        });
        Ok(Proxy {
            shared,
            listen,
            tasks: vec![accept],
        })
    }

    /// Sets what the path does now (existing connections feel it at once).
    pub fn set(&self, p: Path) {
        *self.shared.path.lock().expect("path") = p;
    }

    pub fn path(&self) -> Path {
        self.shared.path.lock().expect("path").clone()
    }

    /// Resets every live connection (a NAT change, a dropped link).
    pub fn reset_all(&self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.stats.resets.fetch_add(1, Ordering::Relaxed);
    }

    /// While on: live connections are reset and new ones refused.
    pub fn sever(&self, on: bool) {
        self.shared.sever.store(on, Ordering::SeqCst);
        if on {
            self.reset_all();
        }
    }

    pub fn stats(&self) -> (u64, u64, u64, u64) {
        let s = &self.shared.stats;
        (
            s.bytes_up.load(Ordering::Relaxed),
            s.bytes_down.load(Ordering::Relaxed),
            s.connections.load(Ordering::Relaxed),
            s.resets.load(Ordering::Relaxed),
        )
    }

    /// Resets the path every `every_ms` (a random value in the range) for
    /// `down_ms`, until the proxy stops. Returns a handle to cancel.
    pub fn flap(&self, every_ms: (u64, u64), down_ms: (u64, u64)) -> tokio::task::JoinHandle<()> {
        let sh = self.shared.clone();
        tokio::spawn(async move {
            let rnd = |r: (u64, u64)| r.0 + rand::random::<u64>() % (r.1.saturating_sub(r.0) + 1);
            while !sh.stop.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(rnd(every_ms))).await;
                sh.sever.store(true, Ordering::SeqCst);
                sh.epoch.fetch_add(1, Ordering::SeqCst);
                sh.stats.resets.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(rnd(down_ms))).await;
                sh.sever.store(false, Ordering::SeqCst);
            }
        })
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn connection(sh: Arc<Shared>, client: TcpStream, target: SocketAddr) -> std::io::Result<()> {
    let upstream = TcpStream::connect(target).await?;
    let _ = client.set_nodelay(true);
    let _ = upstream.set_nodelay(true);
    sh.stats.connections.fetch_add(1, Ordering::Relaxed);
    let epoch = sh.epoch.load(Ordering::SeqCst);
    let (cr, cw) = client.into_split();
    let (ur, uw) = upstream.into_split();
    let a = pump(sh.clone(), cr, uw, true, epoch);
    let b = pump(sh.clone(), ur, cw, false, epoch);
    let _ = tokio::join!(a, b);
    Ok(())
}

/// The delay TCP adds when each packet is lost with probability `p`: a
/// timeout that doubles while the retransmission is lost too.
fn loss_delay(p: f64, rto: Duration) -> Duration {
    if p <= 0.0 {
        return Duration::ZERO;
    }
    let mut d = Duration::ZERO;
    let mut t = rto;
    let mut n = 0;
    while rand::random::<f64>() < p && n < 6 {
        d += t;
        t *= 2;
        n += 1;
    }
    d
}

async fn pump(
    sh: Arc<Shared>,
    mut rd: tokio::net::tcp::OwnedReadHalf,
    mut wr: tokio::net::tcp::OwnedWriteHalf,
    up: bool,
    epoch: u64,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let sh2 = sh.clone();
    let writer = tokio::spawn(async move {
        while let Some((at, bytes)) = rx.recv().await {
            tokio::time::sleep_until(at).await;
            if sh2.epoch.load(Ordering::SeqCst) != epoch {
                break;
            }
            let n = bytes.len() as u64;
            if wr.write_all(&bytes).await.is_err() {
                break;
            }
            let c = if up {
                &sh2.stats.bytes_up
            } else {
                &sh2.stats.bytes_down
            };
            c.fetch_add(n, Ordering::Relaxed);
        }
        let _ = wr.shutdown().await;
    });
    let mut buf = vec![0u8; MSS];
    loop {
        if sh.epoch.load(Ordering::SeqCst) != epoch {
            break;
        }
        let n = tokio::select! {
            r = rd.read(&mut buf) => match r { Ok(0) | Err(_) => break, Ok(n) => n },
            _ = tokio::time::sleep(Duration::from_millis(50)) => continue,
        };
        // Hold: the path is blackholed; wait without reading more.
        loop {
            let s = shape(&sh, up);
            if sh.epoch.load(Ordering::SeqCst) != epoch {
                return;
            }
            if !s.hold {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let s = shape(&sh, up);
        let now = Instant::now();
        let ser = if s.bandwidth > 0 {
            Duration::from_secs_f64(n as f64 / s.bandwidth as f64)
        } else {
            Duration::ZERO
        };
        // The link is shared by every connection through it.
        let start = {
            let mut f = sh.free.lock().expect("free");
            let slot = if up { &mut f.0 } else { &mut f.1 };
            let start = now.max(*slot);
            *slot = start + ser;
            start
        };
        let jitter = if s.jitter_ms > 0 {
            rand::random::<u64>() % (s.jitter_ms + 1)
        } else {
            0
        };
        let latency = Duration::from_millis(s.latency_ms + jitter);
        let rto =
            Duration::from_millis(if s.rto_ms == 0 { 200 } else { s.rto_ms }).max(latency * 2);
        let extra = loss_delay(s.loss, rto);
        // Packets behind a lost one wait for it (head-of-line), so the
        // delay also pushes the serializer.
        if !extra.is_zero() {
            let mut f = sh.free.lock().expect("free");
            let slot = if up { &mut f.0 } else { &mut f.1 };
            *slot += extra;
        }
        let at = start + ser + latency + extra;
        if tx.send((at, buf[..n].to_vec())).is_err() {
            break;
        }
    }
    drop(tx);
    let _ = writer.await;
}

fn shape(sh: &Shared, up: bool) -> Shape {
    let p = sh.path.lock().expect("path");
    if up { p.up.clone() } else { p.down.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn echo() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut b = vec![0u8; 4096];
                    loop {
                        match s.read(&mut b).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                if s.write_all(&b[..n]).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        a
    }

    #[tokio::test]
    async fn latency_hold_and_sever_apply_to_live_connections() {
        let target = echo().await;
        let p = Proxy::start("127.0.0.1:0".parse().unwrap(), target)
            .await
            .unwrap();
        let mut c = TcpStream::connect(p.listen).await.unwrap();
        async fn rt(c: &mut TcpStream) -> Duration {
            let t = std::time::Instant::now();
            c.write_all(b"ping").await.unwrap();
            let mut b = [0u8; 4];
            c.read_exact(&mut b).await.unwrap();
            t.elapsed()
        }
        assert!(rt(&mut c).await < Duration::from_millis(100));
        p.set(Path::both(Shape {
            latency_ms: 150,
            ..Default::default()
        }));
        let t = rt(&mut c).await;
        assert!(t >= Duration::from_millis(280), "{t:?}");
        // Hold: nothing arrives until it is cleared.
        p.set(Path::both(Shape {
            hold: true,
            ..Default::default()
        }));
        c.write_all(b"held").await.unwrap();
        let mut b = [0u8; 4];
        assert!(
            tokio::time::timeout(Duration::from_millis(300), c.read_exact(&mut b))
                .await
                .is_err()
        );
        p.set(Path::default());
        tokio::time::timeout(Duration::from_secs(2), c.read_exact(&mut b))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&b, b"held");
        // Sever resets this connection and refuses the next.
        p.sever(true);
        tokio::time::sleep(Duration::from_millis(150)).await;
        c.write_all(b"x").await.ok();
        let r = tokio::time::timeout(Duration::from_secs(1), c.read(&mut b)).await;
        assert!(matches!(r, Ok(Ok(0)) | Ok(Err(_))), "{r:?}");
        let mut c2 = TcpStream::connect(p.listen).await.unwrap();
        let r = tokio::time::timeout(Duration::from_secs(1), c2.read(&mut b)).await;
        assert!(matches!(r, Ok(Ok(0)) | Ok(Err(_))));
        p.sever(false);
        let mut c3 = TcpStream::connect(p.listen).await.unwrap();
        assert!(rt(&mut c3).await < Duration::from_millis(200));
        p.stop();
    }

    #[test]
    fn loss_adds_doubling_timeouts() {
        assert_eq!(loss_delay(0.0, Duration::from_millis(200)), Duration::ZERO);
        let total: Duration = (0..2000)
            .map(|_| loss_delay(0.4, Duration::from_millis(200)))
            .sum();
        let mean = total / 2000;
        // Expected about rto * (p + 2p^2 + 4p^3 ...) is a few hundred ms.
        assert!(
            mean > Duration::from_millis(100) && mean < Duration::from_millis(1500),
            "{mean:?}"
        );
    }
}
