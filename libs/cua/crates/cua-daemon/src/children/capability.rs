//! The spawn capability: what a Space presents on its spawn channel to make
//! and manage its own children.
//!
//! A capability is `cuaspawn1.<claims>.<mac>`: base64url JSON claims and an
//! HMAC-SHA-256 over them with this host's spawn key (`<home>/spawn/key`,
//! 0600, never leaves the host). It is checked three ways, all required:
//!
//! - the MAC: only this host made it;
//! - the record (`<home>/spawn/capabilities.json`): one live capability
//!   per Space, by its `jti`. Minting again or revoking (the Space is
//!   deleted, its children are turned off) ends the old one at once;
//! - the channel: the Space it names must be the Space whose channel it came
//!   in on. A capability copied out of a snapshot, a fork or another Space
//!   is refused, because the host knows each channel's Space by the
//!   connection it opened itself.
//!
//! It holds no account data, keys or cloud links: it only names a Space,
//! its tree and how deep it sits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Prefix of every capability (and its version).
pub const PREFIX: &str = "cuaspawn1";
/// Domain separation for the MAC.
const MAC_CONTEXT: &[u8] = b"cua-spawn-capability/v1\n";
/// Default lifetime. The host mints a fresh one well before this while the
/// Space runs (each time it attaches the channel, and every
/// [`RENEW_AFTER_SECS`]).
pub const DEFAULT_TTL_SECS: u64 = 24 * 3600;
/// When the host replaces a live capability with a fresh one.
pub const RENEW_AFTER_SECS: u64 = 12 * 3600;
/// Default depth below a Space a person made: its children and their
/// children.
pub const DEFAULT_MAX_DEPTH: u32 = 2;
/// The deepest a tree may go, whatever is asked.
pub const HARD_MAX_DEPTH: u32 = 4;

type HmacSha256 = Hmac<Sha256>;

/// What a capability says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// The Space it belongs to.
    pub space: String,
    /// The root of the Space's tree (itself for a Space a person made).
    pub root: String,
    /// How deep the Space is (0 for a Space a person made).
    pub depth: u32,
    /// The deepest any Space of the tree may be.
    pub max_depth: u32,
    /// Expiry, unix seconds.
    pub exp: u64,
    /// Issued at, unix seconds.
    pub iat: u64,
    /// Unique id; the record holds the live one.
    pub jti: String,
    /// Which host key made it (first 16 hex of SHA-256 of the key).
    pub kid: String,
}

/// Why a capability is refused. Every refusal reads the same to the
/// caller ([`Refusal::message`]); the kind is for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not `cuaspawn1.<claims>.<mac>`.
    Malformed,
    /// The MAC does not match this host's key.
    BadSignature,
    /// Made by another host key.
    OtherHost,
    /// Past `exp`.
    Expired,
    /// Names another Space than the channel's.
    WrongSpace,
    /// Not the Space's live capability (replaced or revoked).
    Revoked,
}

impl Refusal {
    /// The one answer a caller gets.
    pub fn message(self) -> &'static str {
        "this Space's spawn capability is missing, expired or not valid here"
    }
}

/// A live capability's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The live capability's id.
    pub jti: String,
    /// Its tree's root.
    pub root: String,
    /// Its depth.
    pub depth: u32,
    /// Its tree's depth limit.
    pub max_depth: u32,
    /// Issued at, unix seconds.
    pub iat: u64,
    /// Expiry, unix seconds.
    pub exp: u64,
}

/// Mints, checks and revokes capabilities for one host home.
pub struct Authority {
    dir: PathBuf,
    key: [u8; 32],
    kid: String,
    records: Mutex<BTreeMap<String, Record>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .ok()
}

/// Writes `bytes` to `path` atomically, 0600 on Unix.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension(format!("tmp-{:016x}", rand::random::<u64>()));
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

impl Authority {
    /// Opens (or creates) the authority in `<home>/spawn`.
    pub fn open(home: &Path) -> std::io::Result<Self> {
        let dir = home.join("spawn");
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let key_path = dir.join("key");
        let key = match std::fs::read(&key_path) {
            Ok(k) if k.len() == 32 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&k);
                key
            }
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "{} is not a spawn key",
                    key_path.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key: [u8; 32] = rand::random();
                write_private(&key_path, &key)?;
                key
            }
            Err(e) => return Err(e),
        };
        let kid = hex_prefix(&Sha256::digest(key));
        let records = match std::fs::read(dir.join("capabilities.json")) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(_) => BTreeMap::new(),
        };
        Ok(Self {
            dir,
            key,
            kid,
            records: Mutex::new(records),
        })
    }

    fn save(&self, records: &BTreeMap<String, Record>) {
        let bytes = serde_json::to_vec_pretty(records).unwrap_or_default();
        if let Err(e) = write_private(&self.dir.join("capabilities.json"), &bytes) {
            tracing::warn!(error = %e, "could not save the spawn capability records");
        }
    }

    fn mac(&self, payload: &str) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("any key length");
        mac.update(MAC_CONTEXT);
        mac.update(payload.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    /// Mints a capability for `space`, replacing (and so revoking) the one
    /// it had.
    pub fn mint(
        &self,
        space: &str,
        root: &str,
        depth: u32,
        max_depth: u32,
        ttl_secs: u64,
    ) -> String {
        let iat = now();
        let claims = Claims {
            space: space.to_owned(),
            root: root.to_owned(),
            depth,
            max_depth: max_depth.min(HARD_MAX_DEPTH),
            exp: iat.saturating_add(ttl_secs),
            iat,
            jti: format!("{:032x}", rand::random::<u128>()),
            kid: self.kid.clone(),
        };
        let payload = b64(&serde_json::to_vec(&claims).expect("claims serialize"));
        let token = format!("{PREFIX}.{payload}.{}", b64(&self.mac(&payload)));
        let mut records = self.records.lock().unwrap();
        records.insert(
            space.to_owned(),
            Record {
                jti: claims.jti,
                root: claims.root,
                depth: claims.depth,
                max_depth: claims.max_depth,
                iat: claims.iat,
                exp: claims.exp,
            },
        );
        self.save(&records);
        token
    }

    /// Checks `token` as presented on the channel of `channel_space`.
    pub fn verify(&self, token: &str, channel_space: &str) -> Result<Claims, Refusal> {
        let mut parts = token.trim().split('.');
        let (Some(prefix), Some(payload), Some(sig), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Refusal::Malformed);
        };
        if prefix != PREFIX {
            return Err(Refusal::Malformed);
        }
        let sig = unb64(sig).ok_or(Refusal::Malformed)?;
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("any key length");
        mac.update(MAC_CONTEXT);
        mac.update(payload.as_bytes());
        mac.verify_slice(&sig).map_err(|_| Refusal::BadSignature)?;
        let claims: Claims = unb64(payload)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(Refusal::Malformed)?;
        if claims.kid != self.kid {
            return Err(Refusal::OtherHost);
        }
        if claims.exp <= now() {
            return Err(Refusal::Expired);
        }
        if claims.space != channel_space {
            return Err(Refusal::WrongSpace);
        }
        let records = self.records.lock().unwrap();
        match records.get(&claims.space) {
            Some(r) if r.jti == claims.jti => Ok(claims),
            _ => Err(Refusal::Revoked),
        }
    }

    /// The live record of `space`.
    pub fn record(&self, space: &str) -> Option<Record> {
        self.records.lock().unwrap().get(space).cloned()
    }

    /// Ends `space`'s capability. True when it had one.
    pub fn revoke(&self, space: &str) -> bool {
        let mut records = self.records.lock().unwrap();
        let had = records.remove(space).is_some();
        if had {
            self.save(&records);
        }
        had
    }

    /// Spaces holding a live record.
    pub fn spaces(&self) -> Vec<String> {
        self.records.lock().unwrap().keys().cloned().collect()
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority() -> (tempfile::TempDir, Authority) {
        let dir = tempfile::tempdir().unwrap();
        let a = Authority::open(dir.path()).unwrap();
        (dir, a)
    }

    #[test]
    fn a_capability_works_only_on_its_own_spaces_channel() {
        let (_d, a) = authority();
        let cap = a.mint("parent", "parent", 0, 2, 60);
        let claims = a.verify(&cap, "parent").unwrap();
        assert_eq!(
            (claims.space.as_str(), claims.root.as_str(), claims.depth),
            ("parent", "parent", 0)
        );
        // Copied into a sibling, a child or another tree: refused.
        assert_eq!(a.verify(&cap, "sibling"), Err(Refusal::WrongSpace));
        assert_eq!(a.verify(&cap, "parent.c1"), Err(Refusal::WrongSpace));
    }

    #[test]
    fn minting_again_or_revoking_ends_the_old_capability() {
        let (_d, a) = authority();
        let old = a.mint("s", "s", 0, 2, 60);
        let new = a.mint("s", "s", 0, 2, 60);
        assert_eq!(a.verify(&old, "s"), Err(Refusal::Revoked));
        assert!(a.verify(&new, "s").is_ok());
        assert!(a.revoke("s"));
        assert_eq!(a.verify(&new, "s"), Err(Refusal::Revoked));
        assert!(!a.revoke("s"));
    }

    #[test]
    fn a_replayed_capability_from_a_snapshot_is_refused_after_a_fresh_mint() {
        // A memory snapshot holds the capability the guest had; the fork
        // restored from it gets its own, and the parent's replaced one.
        let (_d, a) = authority();
        let in_snapshot = a.mint("parent", "parent", 0, 2, 60);
        let _fork = a.mint("fork", "fork", 0, 2, 60);
        assert_eq!(a.verify(&in_snapshot, "fork"), Err(Refusal::WrongSpace));
        let _renewed = a.mint("parent", "parent", 0, 2, 60);
        assert_eq!(a.verify(&in_snapshot, "parent"), Err(Refusal::Revoked));
    }

    #[test]
    fn tampering_expiry_and_other_hosts_are_refused() {
        let (_d, a) = authority();
        let cap = a.mint("s", "s", 0, 2, 60);
        // Raise the depth limit in the claims, keep the MAC.
        let parts: Vec<&str> = cap.split('.').collect();
        let mut claims: Claims = serde_json::from_slice(&unb64(parts[1]).unwrap()).unwrap();
        claims.max_depth = 4;
        let forged = format!(
            "{PREFIX}.{}.{}",
            b64(&serde_json::to_vec(&claims).unwrap()),
            parts[2]
        );
        assert_eq!(a.verify(&forged, "s"), Err(Refusal::BadSignature));
        assert_eq!(a.verify("nonsense", "s"), Err(Refusal::Malformed));
        assert_eq!(a.verify("cuaspawn1.a.b.c", "s"), Err(Refusal::Malformed));
        let expired = a.mint("e", "e", 0, 2, 0);
        assert_eq!(a.verify(&expired, "e"), Err(Refusal::Expired));
        // Another host's key: its capabilities mean nothing here.
        let (_d2, other) = authority();
        let theirs = other.mint("s", "s", 0, 2, 60);
        assert_eq!(a.verify(&theirs, "s"), Err(Refusal::BadSignature));
    }

    #[test]
    fn the_key_and_records_survive_a_restart_and_the_depth_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let cap = {
            let a = Authority::open(dir.path()).unwrap();
            a.mint("s", "r", 1, 9, 60)
        };
        let a = Authority::open(dir.path()).unwrap();
        let claims = a.verify(&cap, "s").unwrap();
        assert_eq!(claims.max_depth, HARD_MAX_DEPTH);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for f in ["key", "capabilities.json"] {
                let mode = std::fs::metadata(dir.path().join("spawn").join(f))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "{f}");
            }
        }
    }
}
