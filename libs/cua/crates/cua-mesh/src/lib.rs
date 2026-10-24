// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A WireGuard®-based mesh inside Cua, for Spaces traffic only.
//!
//! Every device and machine runs a userspace node ([`Mesh`]): no TUN
//! device, no Network Extension, no kernel module, no root, and no clash
//! with a VPN (Tailscale included) the machine already runs. Mesh addresses
//! (`fd5c:a000::/32`) exist only inside Cua.
//!
//! - **End to end.** Peers talk WireGuard (the Noise protocol state machine
//!   from Cloudflare's BSD-licensed boringtun). The relay never holds a
//!   session key: what it forwards is ciphertext.
//! - **Direct first.** Each node learns its public endpoint over STUN,
//!   publishes its candidates through the coordinator, and pings a peer's
//!   candidates while asking the peer (through the relay) to ping back at
//!   the same time, which opens both NATs ([`engine`]).
//! - **Encrypted fallback.** Until a direct path answers, and whenever it
//!   stops answering, packets go through the coordinator's relay, which
//!   forwards WireGuard and disco ciphertext between the two node keys.
//! - **Streams.** QUIC runs over the tunnel between the two mesh addresses
//!   ([`quic`]); [`Mesh::forward`] turns it into a loopback port any HTTP,
//!   gRPC or WebSocket client can use, and UDP on the same port reaches the
//!   peer's media port (QUIC streaming over the mesh).
//! - **Survives disruption.** Sessions belong to keys and mesh addresses,
//!   not to paths: a coordinator or relay restart, a NAT rebinding or a
//!   network change moves packets to another path underneath a QUIC
//!   connection that keeps going.
//!
//! The coordinator (accounts, devices, share lists, the relay) is the
//! source-available `cua-relay`; this crate only speaks its protocol
//! ([`proto`]). [`testing`] has an in-process stand-in for tests.

pub mod coord;
pub mod dialer;
pub mod disco;
pub mod engine;
pub mod gang;
pub mod key;
pub mod node;
pub mod packet;
pub mod policy;
pub mod proto;
pub mod quic;
pub mod stun;
pub mod vnet;

#[cfg(feature = "testing")]
pub mod testing;

pub use coord::{AuthFuture, AuthProvider, ControlState, PairVerifier};
pub use engine::Path;
pub use key::{NodeIdentity, PublicKey, SecretKey};
pub use node::{Forward, IncomingStream, MEDIA_UDP_PORT, Mesh, MeshConfig, PeerStatus, Status};
pub use policy::Policy;
pub use proto::{Grant, MeshInfo, Mode, NodeKind, PeerInfo, PeerUsage, VERSION};
pub use quic::{MeshStream, STREAM_PORT, StreamHeader};

/// Mesh errors.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// Local I/O or setup.
    #[error("mesh: {0}")]
    Io(String),
    /// A malformed or unexpected message.
    #[error("mesh protocol: {0}")]
    Protocol(String),
    /// The coordinator or peer is out of reach.
    #[error("mesh unreachable: {0}")]
    Unreachable(String),
    /// Refused (credentials, policy, a pairing proof).
    #[error("mesh refused: {0}")]
    Refused(String),
    /// The other side does not support the mesh (or this version of it).
    #[error("mesh unsupported: {0}")]
    Unsupported(String),
    /// No such peer.
    #[error("mesh: {0}")]
    NotFound(String),
}

/// Mesh results.
pub type Result<T> = std::result::Result<T, Error>;

/// An [`AuthProvider`] that always sends the same headers.
pub fn static_auth(headers: Vec<(String, String)>) -> AuthProvider {
    std::sync::Arc::new(move || {
        let h = headers.clone();
        Box::pin(async move { Ok(h) })
    })
}
