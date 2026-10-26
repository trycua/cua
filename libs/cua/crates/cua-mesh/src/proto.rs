// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The coordinator protocol: one WebSocket per node (`GET /v1/mesh/connect`)
//! carries JSON control messages (text frames) and relayed packets (binary
//! frames).
//!
//! ```text
//! node                                  coordinator
//!   | ---- WebSocket upgrade (credentials) --> |
//!   | <--- challenge {server_key, nonce} ----- |
//!   | ---- hello {keys, endpoints, proof} ---> |   proof: HMAC over the
//!   | <--- welcome {addr, mode, stun} -------- |   nonce keyed by
//!   | <--- netmap {peers} -------------------- |   X25519(node, server)
//!   | ---- endpoints {...} ------------------> |   (proves the node key)
//!   | <=== binary: RECV src(32) | packet ===== |
//!   | ==== binary: SEND dst(32) | packet ====> |   (WireGuard or disco
//! ```                                              ciphertext only)
//!
//! The coordinator implementation (accounts, devices, sharing and the
//! relay) is the source-available `cua-relay`; this module is the contract
//! both sides speak.

use std::net::{Ipv6Addr, SocketAddr};

use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::key::{PublicKey, SecretKey};

/// The mesh protocol version this crate speaks. A coordinator and a peer
/// agree on the mesh when their versions match; anything else falls back to
/// the relay's HTTPS path.
pub const VERSION: u32 = 1;

/// WebSocket close codes the coordinator gives when it ends a session
/// itself (the private-use range), so a node can say why it was dropped and
/// whether trying again can help. The close reason is a short English
/// sentence for logs.
///
/// The relay restarts or its state changed: reconnect (spread out).
pub const CLOSE_RELAY_CHANGED: u16 = 4000;
/// Another connection with the same node key took over this one. A node
/// that keeps hearing this shares its identity with another running copy.
pub const CLOSE_REPLACED: u16 = 4001;
/// The device behind this node is no longer enrolled.
pub const CLOSE_DEVICE: u16 = 4002;
/// The session reached its maximum age: reconnect with a fresh token.
pub const CLOSE_SESSION_MAX: u16 = 4003;
/// The account or machine was erased.
pub const CLOSE_ERASED: u16 = 4004;
/// This node connected too often; the reason is `retry after N s`
/// ([`retry_after_reason`], [`parse_retry_after`]).
pub const CLOSE_RATE_LIMITED: u16 = 4029;

/// The close reason that tells a node when it may connect again.
pub fn retry_after_reason(secs: u64) -> String {
    format!("too many reconnects: retry after {secs} s")
}

/// The seconds in a [`retry_after_reason`] (or any `retry after N` text).
pub fn parse_retry_after(reason: &str) -> Option<u64> {
    let rest = reason.split("retry after ").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The coordinator's WebSocket path.
pub const CONNECT_PATH: &str = "/v1/mesh/connect";

/// Header naming a machine's id on its connect (with its machine token or
/// a static registration token as the bearer).
pub const MACHINE_ID_HEADER: &str = "x-cua-machine-id";

/// Binary frame: node to coordinator, `dst key | packet`.
pub const FRAME_SEND: u8 = 1;
/// Binary frame: coordinator to node, `src key | packet`.
pub const FRAME_RECV: u8 = 2;

/// Encodes a relay frame.
pub fn frame(kind: u8, key: &PublicKey, packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(33 + packet.len());
    out.push(kind);
    out.extend_from_slice(&key.0);
    out.extend_from_slice(packet);
    out
}

/// Decodes a relay frame.
pub fn parse_frame(b: &[u8]) -> Option<(u8, PublicKey, &[u8])> {
    if b.len() < 33 {
        return None;
    }
    Some((b[0], PublicKey(b[1..33].try_into().ok()?), &b[33..]))
}

/// The largest relayed packet the coordinator accepts.
pub const MAX_RELAY_PACKET: usize = 64 * 1024;

/// What kind of node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A hosted machine (`cua-spacesd join`): serves Spaces.
    Machine,
    /// A client device (the cua daemon, CLI, SDK or app).
    Client,
}

/// How the coordinator authorizes peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// cua.ai-style accounts: owners, share lists and enrolled devices.
    Account,
    /// Self-hosted static tokens: each client pairs with a machine by
    /// proving it holds that machine's env token; the machine approves.
    Static,
}

/// The [`Grant::role`] of a peer that is another machine of the same
/// account (machine-to-machine peering: a gang's rank traffic between the
/// owner's hosts). Such a peer reaches only [`port_service`] streams a
/// running job registered, never a machine's spacesd.
pub const ROLE_MACHINE: &str = "machine";

/// The stream service name for local TCP port `port` on a machine
/// (`port:<n>`; see [`parse_port_service`]).
pub fn port_service(port: u16) -> String {
    format!("port:{port}")
}

/// The port of a [`port_service`] name (`port:<n>`, `n` in 1..=65535).
pub fn parse_port_service(service: &str) -> Option<u16> {
    let n = service.strip_prefix("port:")?;
    if n.is_empty() || n.len() > 5 || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok().filter(|p| *p != 0)
}

/// Who a client peer is, as the coordinator vouches to a machine (account
/// mode). The machine turns it into the same identity a relay-signed
/// assertion carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Grant {
    /// User id (`sub`).
    pub user: String,
    /// Account id.
    pub account: String,
    /// Verified email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `owner`, `shared` or `viewer`.
    pub role: String,
    /// The enrolled device, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

/// A peer in a node's network map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// WireGuard public key.
    pub node_key: PublicKey,
    /// Disco public key.
    pub disco_key: PublicKey,
    /// SHA-256 (hex) of the peer's QUIC certificate.
    pub quic_pin: String,
    /// Machine or client.
    pub kind: NodeKind,
    /// The machine id (machines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Candidate UDP endpoints (LAN addresses and STUN-learned public ones).
    #[serde(default)]
    pub endpoints: Vec<SocketAddr>,
    /// Connected to the coordinator right now.
    #[serde(default)]
    pub online: bool,
    /// The peer's mesh protocol version.
    pub version: u32,
    /// Who the peer is (in a machine's map, for client peers in account
    /// mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Grant>,
}

impl PeerInfo {
    /// The peer's mesh address.
    pub fn addr(&self) -> Ipv6Addr {
        self.node_key.mesh_addr()
    }
}

/// Coordinator to node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// First message: prove the node key.
    Challenge {
        /// Coordinator's protocol version.
        version: u32,
        /// Ephemeral X25519 key of this connection.
        server_key: PublicKey,
        /// Random nonce (hex).
        nonce: String,
    },
    /// Accepted.
    Welcome {
        /// Coordinator's protocol version.
        version: u32,
        /// This node's mesh address.
        addr: Ipv6Addr,
        /// Account or static.
        mode: Mode,
        /// STUN servers (`host:port`).
        #[serde(default)]
        stun: Vec<String>,
        /// The newest [`ClientMsg::Usage`] version this coordinator meters
        /// (0, or absent from an older one: none, so machines send none).
        #[serde(default, skip_serializing_if = "is_zero")]
        usage: u32,
    },
    /// The full set of peers this node may talk to.
    Netmap {
        /// Peers.
        peers: Vec<PeerInfo>,
    },
    /// Static mode, to a machine: a client asks to pair.
    PairRequest {
        /// Echo in the reply.
        request_id: String,
        /// The client's node key.
        client_key: PublicKey,
        /// Unix seconds the proof was made.
        ts: u64,
        /// HMAC proof (hex), see [`pair_proof`].
        proof: String,
        /// The client's name.
        #[serde(default)]
        name: String,
    },
    /// Static mode, to a client: how a pairing went.
    PairResult {
        /// The machine.
        machine_id: String,
        /// Paired.
        ok: bool,
        /// Why not.
        #[serde(default)]
        reason: String,
    },
    /// Keepalive answer.
    Pong,
    /// Refused; the connection closes.
    Error {
        /// Why.
        message: String,
    },
}

/// Node to coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Answer to the challenge.
    Hello {
        /// Node's protocol version.
        version: u32,
        /// Machine or client.
        kind: NodeKind,
        /// WireGuard public key.
        node_key: PublicKey,
        /// Disco public key.
        disco_key: PublicKey,
        /// SHA-256 (hex) of the QUIC certificate.
        quic_pin: String,
        /// Display name.
        #[serde(default)]
        name: String,
        /// Candidate endpoints.
        #[serde(default)]
        endpoints: Vec<SocketAddr>,
        /// HMAC proof (hex), see [`hello_proof`].
        proof: String,
        /// A short-lived node (one CLI run): forgotten when it disconnects.
        #[serde(default)]
        ephemeral: bool,
    },
    /// The node's endpoints changed.
    Endpoints {
        /// Candidate endpoints.
        endpoints: Vec<SocketAddr>,
    },
    /// Static mode, from a client: pair with a machine.
    Pair {
        /// The machine.
        machine_id: String,
        /// Unix seconds.
        ts: u64,
        /// HMAC proof (hex), see [`pair_proof`].
        proof: String,
    },
    /// Static mode, from a machine: the answer to a pair request.
    PairReply {
        /// The request.
        request_id: String,
        /// Accepted.
        accept: bool,
        /// Why not.
        #[serde(default)]
        reason: String,
    },
    /// Keepalive.
    Ping,
    /// Account mode, from a machine: the streams its peers hold open now,
    /// so the coordinator can meter them as it meters its own HTTPS path.
    /// A full snapshot, sent when it changes (and again after a
    /// reconnect), only to a coordinator whose welcome asked for
    /// version `v`. A coordinator ignores versions it does not know.
    Usage {
        /// Report version ([`USAGE_VERSION`]).
        v: u32,
        /// One entry per peer with something open (or just upgraded).
        #[serde(default)]
        peers: Vec<PeerUsage>,
    },
}

/// The [`ClientMsg::Usage`] version this crate sends.
pub const USAGE_VERSION: u32 = 1;

/// The most entries one [`ClientMsg::Usage`] carries.
pub const MAX_USAGE_PEERS: usize = 256;

/// One peer's streams in a [`ClientMsg::Usage`] report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerUsage {
    /// The client peer's node key (the coordinator knows its account).
    pub peer: PublicKey,
    /// The user its grant named (`sub`); the coordinator checks it against
    /// the peer's own account and ignores a mismatch.
    pub user: String,
    /// Streams the peer holds open now (HTTP requests in flight, upgraded
    /// WebSockets, active media flows).
    pub open: u32,
    /// A WebSocket upgrade or a media flow since the last report: a session
    /// at once, whatever its length.
    #[serde(default)]
    pub live: bool,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

type HmacSha256 = Hmac<Sha256>;

/// The hello proof: `HMAC-SHA256(key = X25519(node, server_key), "cua-mesh-hello-v1" | nonce | node key)`.
pub fn hello_proof(node: &SecretKey, server_key: &PublicKey, nonce: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(&node.agree(server_key)).expect("any key length");
    mac.update(b"cua-mesh-hello-v1");
    mac.update(nonce.as_bytes());
    mac.update(&node.public().0);
    hex::encode(mac.finalize().into_bytes())
}

/// Checks a hello proof on the coordinator (`server` is the connection's
/// ephemeral secret).
pub fn verify_hello_proof(
    server: &SecretKey,
    node_key: &PublicKey,
    nonce: &str,
    proof: &str,
) -> bool {
    let Ok(proof) = hex::decode(proof) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(&server.agree(node_key)).expect("any key length");
    mac.update(b"cua-mesh-hello-v1");
    mac.update(nonce.as_bytes());
    mac.update(&node_key.0);
    mac.verify_slice(&proof).is_ok()
}

/// How long a pairing proof stays valid.
pub const PAIR_PROOF_TTL_SECS: u64 = 300;

/// The static-mode pairing proof a client sends to pair with a machine:
/// `HMAC-SHA256(key = the machine's env token, "cua-mesh-pair-v1" | machine id | client node key | ts)`.
/// The coordinator only forwards it; the machine checks it against its own
/// env token, so the coordinator never sees the token.
pub fn pair_proof(env_token: &str, machine_id: &str, client: &PublicKey, ts: u64) -> String {
    let mut mac = HmacSha256::new_from_slice(env_token.as_bytes()).expect("any key length");
    mac.update(b"cua-mesh-pair-v1|");
    mac.update(machine_id.as_bytes());
    mac.update(b"|");
    mac.update(&client.0);
    mac.update(&ts.to_be_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Checks a pairing proof on the machine.
pub fn verify_pair_proof(
    env_token: &str,
    machine_id: &str,
    client: &PublicKey,
    ts: u64,
    proof: &str,
    now: u64,
) -> bool {
    if ts + PAIR_PROOF_TTL_SECS < now || ts > now + 60 {
        return false;
    }
    let Ok(proof) = hex::decode(proof) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(env_token.as_bytes()).expect("any key length");
    mac.update(b"cua-mesh-pair-v1|");
    mac.update(machine_id.as_bytes());
    mac.update(b"|");
    mac.update(&client.0);
    mac.update(&ts.to_be_bytes());
    mac.verify_slice(&proof).is_ok()
}

/// Unix seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What a relay advertises about its mesh in `GET /v1/info` (`mesh`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshInfo {
    /// Protocol version.
    pub version: u32,
    /// STUN servers.
    #[serde(default)]
    pub stun: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_services_round_trip_and_reject_anything_else() {
        assert_eq!(port_service(47012), "port:47012");
        assert_eq!(parse_port_service("port:47012"), Some(47012));
        assert_eq!(parse_port_service("port:65535"), Some(65535));
        for bad in [
            "port:0",
            "port:",
            "port:65536",
            "port:-1",
            "port:+80",
            "port: 80",
            "port:080000",
            "spacesd",
            "Port:80",
            "port:80x",
        ] {
            assert_eq!(parse_port_service(bad), None, "{bad}");
        }
    }

    #[test]
    fn hello_proof_binds_the_node_key() {
        let node = SecretKey::generate();
        let server = SecretKey::generate();
        let p = hello_proof(&node, &server.public(), "n1");
        assert!(verify_hello_proof(&server, &node.public(), "n1", &p));
        assert!(!verify_hello_proof(&server, &node.public(), "n2", &p));
        let other = SecretKey::generate().public();
        assert!(!verify_hello_proof(&server, &other, "n1", &p));
    }

    #[test]
    fn pair_proof_checks_token_machine_and_time() {
        let client = SecretKey::generate().public();
        let now = 1_800_000_000;
        let p = pair_proof("env-token", "machine-1", &client, now);
        assert!(verify_pair_proof(
            "env-token",
            "machine-1",
            &client,
            now,
            &p,
            now + 5
        ));
        assert!(!verify_pair_proof(
            "other",
            "machine-1",
            &client,
            now,
            &p,
            now
        ));
        assert!(!verify_pair_proof(
            "env-token",
            "machine-2",
            &client,
            now,
            &p,
            now
        ));
        assert!(!verify_pair_proof(
            "env-token",
            "machine-1",
            &client,
            now,
            &p,
            now + PAIR_PROOF_TTL_SECS + 1
        ));
    }

    #[test]
    fn messages_round_trip() {
        let k = SecretKey::generate().public();
        let m = ServerMsg::Netmap {
            peers: vec![PeerInfo {
                node_key: k,
                disco_key: k,
                quic_pin: "ab".into(),
                kind: NodeKind::Machine,
                machine_id: Some("m1".into()),
                name: "mac".into(),
                endpoints: vec!["1.2.3.4:5".parse().unwrap()],
                online: true,
                version: VERSION,
                grant: None,
            }],
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"netmap\""));
        assert_eq!(serde_json::from_str::<ServerMsg>(&json).unwrap(), m);
        let f = frame(FRAME_SEND, &k, b"abc");
        assert_eq!(parse_frame(&f), Some((FRAME_SEND, k, &b"abc"[..])));
    }

    #[test]
    fn usage_is_additive_to_the_protocol() {
        // A welcome from an older coordinator (no `usage`) asks for none.
        let old =
            r#"{"type":"welcome","version":1,"addr":"fd5c:a000::1","mode":"account","stun":[]}"#;
        match serde_json::from_str::<ServerMsg>(old).unwrap() {
            ServerMsg::Welcome { usage, .. } => assert_eq!(usage, 0),
            other => panic!("{other:?}"),
        }
        // A newer one says which version it meters; zero is left out.
        let w = ServerMsg::Welcome {
            version: VERSION,
            addr: "fd5c:a000::1".parse().unwrap(),
            mode: Mode::Account,
            stun: vec![],
            usage: 0,
        };
        assert!(!serde_json::to_string(&w).unwrap().contains("usage"));
        let k = SecretKey::generate().public();
        let m = ClientMsg::Usage {
            v: USAGE_VERSION,
            peers: vec![PeerUsage {
                peer: k,
                user: "ada".into(),
                open: 2,
                live: true,
            }],
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"usage\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMsg>(&json).unwrap(), m);
        // A future version still parses (and is ignored by its version).
        let future = r#"{"type":"usage","v":9,"peers":[],"extra":1}"#;
        assert!(matches!(
            serde_json::from_str::<ClientMsg>(future).unwrap(),
            ClientMsg::Usage { v: 9, .. }
        ));
    }
}
