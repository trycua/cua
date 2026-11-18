// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! An in-memory network between named machines, for simulations and the
//! chaos suite. Streams behave like TCP: reliable and ordered, so loss shows
//! up the way it does on a real connection, as delay. What it can do to a
//! pair of machines, per direction:
//!
//! - latency, jitter, and a bandwidth cap;
//! - *hold*: bytes stop arriving (a blackholed path) and flow again when the
//!   hold ends;
//! - *sever*: every stream between the pair is reset and new dials fail
//!   until it is lifted (a long partition or a changed address);
//! - an asymmetric version of any of these.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::link::Stream;
use super::transport::Transport;

/// What one direction of a pair does to traffic.
#[derive(Clone, Debug, Default)]
pub struct Impair {
    pub latency: Duration,
    pub jitter: Duration,
    /// Bytes per second (0: unlimited).
    pub bandwidth: u64,
    /// Bytes stop arriving until this is cleared.
    pub hold: bool,
    /// Streams are reset and dials fail.
    pub sever: bool,
}

type Pair = (String, String);

#[derive(Default)]
struct Net {
    impair: HashMap<Pair, Impair>,
    /// Accept queues by machine.
    inbox: HashMap<String, tokio::sync::mpsc::UnboundedSender<(String, Stream)>>,
    bytes: HashMap<Pair, u64>,
    /// Machines whose process is gone: their connections are reset.
    dead: std::collections::HashSet<String>,
}

/// The network.
#[derive(Clone, Default)]
pub struct SimNet {
    net: Arc<Mutex<Net>>,
}

impl SimNet {
    pub fn new() -> SimNet {
        SimNet::default()
    }

    /// Registers `machine` and returns its transport and the streams that
    /// arrive for it as `(dialer, stream)`.
    pub fn join(
        &self,
        machine: &str,
    ) -> (
        SimTransport,
        tokio::sync::mpsc::UnboundedReceiver<(String, Stream)>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.net
            .lock()
            .expect("net")
            .inbox
            .insert(machine.to_string(), tx);
        (
            SimTransport {
                net: self.clone(),
                me: machine.to_string(),
            },
            rx,
        )
    }

    /// The machine's process is gone: its connections are reset and dials to
    /// it are refused, as when a process dies on a machine that stays up.
    pub fn leave(&self, machine: &str) {
        let mut n = self.net.lock().expect("net");
        n.inbox.remove(machine);
        n.dead.insert(machine.to_string());
    }

    fn is_dead(&self, machine: &str) -> bool {
        self.net.lock().expect("net").dead.contains(machine)
    }

    /// Sets what traffic from `from` to `to` suffers.
    pub fn set(&self, from: &str, to: &str, i: Impair) {
        self.net
            .lock()
            .expect("net")
            .impair
            .insert((from.into(), to.into()), i);
    }

    /// Both directions at once.
    pub fn set_both(&self, a: &str, b: &str, i: Impair) {
        self.set(a, b, i.clone());
        self.set(b, a, i);
    }

    /// Clears everything between the pair.
    pub fn heal(&self, a: &str, b: &str) {
        self.set_both(a, b, Impair::default());
    }

    /// Bytes that crossed from `from` to `to` so far.
    pub fn bytes(&self, from: &str, to: &str) -> u64 {
        *self
            .net
            .lock()
            .expect("net")
            .bytes
            .get(&(from.to_string(), to.to_string()))
            .unwrap_or(&0)
    }

    fn get(&self, from: &str, to: &str) -> Impair {
        self.net
            .lock()
            .expect("net")
            .impair
            .get(&(from.to_string(), to.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn count(&self, from: &str, to: &str, n: u64) {
        *self
            .net
            .lock()
            .expect("net")
            .bytes
            .entry((from.to_string(), to.to_string()))
            .or_default() += n;
    }

    /// One direction: reads from `rd`, delivers to `wr` after the pair's
    /// current impairment. Delays are release times, not sleeps in the read
    /// loop, so a delayed stream still pipelines like a real long path.
    fn pump(
        &self,
        from: String,
        to: String,
        mut rd: tokio::io::ReadHalf<tokio::io::DuplexStream>,
        mut wr: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let net = self.clone();
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
        let (f2, t2, n2) = (from.clone(), to.clone(), net.clone());
        tokio::spawn(async move {
            while let Some((at, bytes)) = rx.recv().await {
                tokio::time::sleep_until(at).await;
                n2.count(&f2, &t2, bytes.len() as u64);
                if wr.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            let mut next_free = tokio::time::Instant::now();
            loop {
                let n = match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                loop {
                    let i = net.get(&from, &to);
                    if i.sever || net.is_dead(&from) || net.is_dead(&to) {
                        return;
                    }
                    if !i.hold {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let i = net.get(&from, &to);
                let now = tokio::time::Instant::now();
                let start = now.max(next_free);
                let ser = if i.bandwidth > 0 {
                    Duration::from_secs_f64(n as f64 / i.bandwidth as f64)
                } else {
                    Duration::ZERO
                };
                next_free = start + ser;
                let mut delay = i.latency;
                if !i.jitter.is_zero() {
                    let j = rand::random::<u64>() % (i.jitter.as_millis() as u64 + 1);
                    delay += Duration::from_millis(j);
                }
                let at = (start + ser + delay).max(now);
                if tx.send((at, buf[..n].to_vec())).is_err() {
                    break;
                }
            }
        });
    }
}

/// One machine's dialer.
#[derive(Clone)]
pub struct SimTransport {
    net: SimNet,
    me: String,
}

#[async_trait::async_trait]
impl Transport for SimTransport {
    async fn dial(&self, machine: &str) -> std::io::Result<Stream> {
        let i = self.net.get(&self.me, machine);
        let back = self.net.get(machine, &self.me);
        if i.sever || back.sever || i.hold || back.hold {
            // A dial into a dead path takes a while to fail.
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no route to machine",
            ));
        }
        let tx = self
            .net
            .net
            .lock()
            .expect("net")
            .inbox
            .get(machine)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "machine is off")
            })?;
        // Two pipes and a pump per direction make one connection.
        let (dialer_end, a_inner) = tokio::io::duplex(256 * 1024);
        let (acceptor_end, b_inner) = tokio::io::duplex(256 * 1024);
        let (a_rd, a_wr) = tokio::io::split(a_inner);
        let (b_rd, b_wr) = tokio::io::split(b_inner);
        // The dial itself costs a round trip.
        tokio::time::sleep(i.latency + back.latency).await;
        self.net
            .pump(self.me.clone(), machine.to_string(), a_rd, b_wr);
        self.net
            .pump(machine.to_string(), self.me.clone(), b_rd, a_wr);
        tx.send((self.me.clone(), Box::new(acceptor_end)))
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "machine is off")
            })?;
        Ok(Box::new(dialer_end))
    }
}
