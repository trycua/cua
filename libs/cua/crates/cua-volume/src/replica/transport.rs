// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! How machines reach each other.
//!
//! In the product a peer link is a stream over the Cua mesh (WireGuard
//! between the account's node keys; the relay forwards ciphertext), so data
//! only ever moves between the user's own signed-in machines and is
//! encrypted. [`Transport`] is the seam: the node asks it for a stream to a
//! machine id and hands it every stream that arrives. [`TcpTransport`] is
//! the same thing over plain TCP for LANs and tests, and must be wrapped in
//! something that encrypts (an SSH tunnel, the mesh) outside a test.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::link::Stream;

/// Opens streams to machines.
#[async_trait::async_trait]
pub trait Transport: Send + Sync + 'static {
    /// A stream to `machine` (its machine id).
    async fn dial(&self, machine: &str) -> std::io::Result<Stream>;
}

/// Plain TCP, addresses by machine id.
#[derive(Clone, Default)]
pub struct TcpTransport {
    addrs: Arc<Mutex<HashMap<String, SocketAddr>>>,
    timeout: Duration,
}

impl TcpTransport {
    pub fn new() -> TcpTransport {
        TcpTransport {
            addrs: Arc::default(),
            timeout: Duration::from_secs(5),
        }
    }

    /// Where `machine` listens.
    pub fn set(&self, machine: &str, addr: SocketAddr) {
        self.addrs
            .lock()
            .expect("addrs")
            .insert(machine.into(), addr);
    }
}

#[async_trait::async_trait]
impl Transport for TcpTransport {
    async fn dial(&self, machine: &str) -> std::io::Result<Stream> {
        let addr = self
            .addrs
            .lock()
            .expect("addrs")
            .get(machine)
            .copied()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no address for {machine}"),
                )
            })?;
        let s = tokio::time::timeout(self.timeout, tokio::net::TcpStream::connect(addr))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out")
            })??;
        s.set_nodelay(true)?;
        Ok(Box::new(s))
    }
}
