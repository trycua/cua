// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Node identity: the WireGuard static key, the disco key, the QUIC
//! certificate, and the mesh address derived from the node key.

use std::fmt;
use std::net::Ipv6Addr;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use boringtun::x25519;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{Error, Result};

/// The mesh's private address prefix (`fd5c:a000::/32`, a unique local
/// range). Every node's address is the prefix plus 96 bits of the SHA-256 of
/// its node key, so addresses need no allocation and a peer's address is
/// checked against its key. They exist only inside Cua: no OS interface ever
/// carries them, so they cannot clash with a VPN the user already runs.
pub const MESH_PREFIX: [u8; 4] = [0xfd, 0x5c, 0xa0, 0x00];

/// A 32-byte public key (a node key or a disco key), base64 on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    /// Parses standard base64.
    pub fn parse(s: &str) -> Result<Self> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|e| Error::Protocol(format!("bad key {s:?}: {e}")))?;
        let bytes: [u8; 32] = raw
            .try_into()
            .map_err(|_| Error::Protocol(format!("bad key length {s:?}")))?;
        Ok(Self(bytes))
    }

    /// Standard base64.
    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }

    /// A short form for logs (`ab12cd34`).
    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }

    /// The mesh address of the node with this node key.
    pub fn mesh_addr(&self) -> Ipv6Addr {
        let digest = Sha256::digest(self.0);
        let mut octets = [0u8; 16];
        octets[..4].copy_from_slice(&MESH_PREFIX);
        octets[4..].copy_from_slice(&digest[..12]);
        Ipv6Addr::from(octets)
    }

    pub(crate) fn x25519(&self) -> x25519::PublicKey {
        x25519::PublicKey::from(self.0)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.short())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64())
    }
}

impl Serialize for PublicKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_base64())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// True when `addr` is in the mesh prefix.
pub fn is_mesh_addr(addr: &Ipv6Addr) -> bool {
    addr.octets()[..4] == MESH_PREFIX
}

/// An X25519 secret (a node key or a disco key).
#[derive(Clone)]
pub struct SecretKey(x25519::StaticSecret);

impl SecretKey {
    /// A fresh random key, from the OS RNG.
    pub fn generate() -> Self {
        Self(x25519::StaticSecret::from(os_random::<32>()))
    }

    /// From raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(x25519::StaticSecret::from(bytes))
    }

    /// The public half.
    pub fn public(&self) -> PublicKey {
        PublicKey(x25519::PublicKey::from(&self.0).to_bytes())
    }

    /// X25519 with `peer`.
    pub fn agree(&self, peer: &PublicKey) -> [u8; 32] {
        self.0.diffie_hellman(&peer.x25519()).to_bytes()
    }

    pub(crate) fn x25519(&self) -> x25519::StaticSecret {
        self.0.clone()
    }

    fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKey(public {})", self.public().short())
    }
}

/// Everything a node keeps across restarts: its node key (WireGuard static
/// key), disco key and QUIC certificate. Stored as one `0600` JSON file.
#[derive(Clone)]
pub struct NodeIdentity {
    /// WireGuard static key.
    pub node: SecretKey,
    /// Path-discovery key (separate from the node key, as in other
    /// WireGuard meshes, so disco never reuses the WireGuard key).
    pub disco: SecretKey,
    /// Self-signed QUIC certificate (DER) peers pin by SHA-256.
    pub cert_der: Vec<u8>,
    /// Its PKCS#8 private key (DER).
    pub cert_key_der: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    version: u32,
    node: String,
    disco: String,
    cert: String,
    cert_key: String,
}

impl fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeIdentity")
            .field("node", &self.node.public().short())
            .field("addr", &self.addr())
            .finish()
    }
}

impl NodeIdentity {
    /// A fresh identity.
    pub fn generate() -> Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec!["cua-mesh".to_owned()])
            .map_err(|e| Error::Io(format!("certificate: {e}")))?;
        Ok(Self {
            node: SecretKey::generate(),
            disco: SecretKey::generate(),
            cert_der: cert.cert.der().to_vec(),
            cert_key_der: cert.signing_key.serialize_der(),
        })
    }

    /// Loads `path`, creating (and saving, `0600`) a fresh identity on
    /// first use.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if let Ok(raw) = std::fs::read_to_string(path) {
            return Self::from_json(&raw)
                .map_err(|e| Error::Io(format!("{}: {e}", path.display())));
        }
        let id = Self::generate()?;
        id.save(path)?;
        Ok(id)
    }

    /// Saves to `path` (`0600`, atomically).
    pub fn save(&self, path: &Path) -> Result<()> {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let stored = StoredIdentity {
            version: 1,
            node: b64(&self.node.to_bytes()),
            disco: b64(&self.disco.to_bytes()),
            cert: b64(&self.cert_der),
            cert_key: b64(&self.cert_key_der),
        };
        let json = serde_json::to_vec_pretty(&stored).map_err(|e| Error::Io(e.to_string()))?;
        write_private(path, &json)
    }

    fn from_json(raw: &str) -> Result<Self> {
        let s: StoredIdentity =
            serde_json::from_str(raw).map_err(|e| Error::Protocol(e.to_string()))?;
        let d = |v: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(v)
                .map_err(|e| Error::Protocol(e.to_string()))
        };
        let key = |v: &str| -> Result<SecretKey> {
            let raw: [u8; 32] = d(v)?
                .try_into()
                .map_err(|_| Error::Protocol("bad key length".into()))?;
            Ok(SecretKey::from_bytes(raw))
        };
        Ok(Self {
            node: key(&s.node)?,
            disco: key(&s.disco)?,
            cert_der: d(&s.cert)?,
            cert_key_der: d(&s.cert_key)?,
        })
    }

    /// The node's public key.
    pub fn public(&self) -> PublicKey {
        self.node.public()
    }

    /// The node's mesh address.
    pub fn addr(&self) -> Ipv6Addr {
        self.public().mesh_addr()
    }

    /// SHA-256 of the QUIC certificate, hex (what peers pin).
    pub fn cert_pin(&self) -> String {
        hex::encode(Sha256::digest(&self.cert_der))
    }
}

/// Writes `bytes` to `path` with mode `0600` (atomically: temp + rename).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
    }
    let tmp: PathBuf = path.with_extension(format!("tmp{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .map_err(|e| Error::Io(format!("{}: {e}", tmp.display())))?;
    f.write_all(bytes).map_err(|e| Error::Io(e.to_string()))?;
    f.sync_all().ok();
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))
}

/// `N` bytes from the OS RNG. Keys and nonces come from here, never from a
/// userspace PRNG (`rand::random`): its state is copied by a memory snapshot
/// into every clone of a running machine.
pub fn os_random<const N: usize>() -> [u8; N] {
    use rand::TryRngCore as _;
    let mut bytes = [0u8; N];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS random number generator");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_addresses_follow_the_key() {
        let k = SecretKey::generate().public();
        let a = k.mesh_addr();
        assert!(is_mesh_addr(&a));
        assert_eq!(a, k.mesh_addr());
        assert_ne!(a, SecretKey::generate().public().mesh_addr());
    }

    #[test]
    fn keys_round_trip() {
        let k = SecretKey::generate().public();
        assert_eq!(PublicKey::parse(&k.to_base64()).unwrap(), k);
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), k);
        assert!(PublicKey::parse("AAAA").is_err());
    }

    #[test]
    fn agreement_is_symmetric() {
        let a = SecretKey::generate();
        let b = SecretKey::generate();
        assert_eq!(a.agree(&b.public()), b.agree(&a.public()));
    }

    #[test]
    fn identity_persists_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mesh/identity.json");
        let a = NodeIdentity::load_or_create(&path).unwrap();
        let b = NodeIdentity::load_or_create(&path).unwrap();
        assert_eq!(a.public(), b.public());
        assert_eq!(a.disco.public(), b.disco.public());
        assert_eq!(a.cert_pin(), b.cert_pin());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
