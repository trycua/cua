//! The anonymous install id and the per-install salt.
//!
//! - `install_id`: 128 random bits, hex. It is the PostHog `distinct_id`.
//!   It is created on the first event that is actually sent, never when
//!   telemetry is off, and never derived from the machine or the user.
//! - `salt`: 256 random bits, hex, never sent. [`Identity::hash`] uses it so
//!   an identifier we must group by (for example a Space) becomes a value
//!   that means nothing outside this install.
//!
//! `cua telemetry reset-id` deletes both; the next event gets fresh ones.

use rand::RngCore;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::fsutil;

/// The install id file.
pub const ID_FILE: &str = "install_id";
/// The salt file.
pub const SALT_FILE: &str = "salt";

/// The loaded identity.
#[derive(Clone, Debug)]
pub struct Identity {
    /// The install id (32 hex).
    pub id: String,
    salt: [u8; 32],
    /// Whether it is stored on disk (false when the home is not writable:
    /// then it lives for this process only).
    pub persisted: bool,
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn valid_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn load_or_create(path: &Path, bytes: usize) -> Option<String> {
    if let Ok(s) = std::fs::read_to_string(path) {
        let s = s.trim().to_string();
        if valid_hex(&s, bytes * 2) {
            return Some(s);
        }
        // Corrupt: replace it.
        let _ = fsutil::remove(path);
    }
    let fresh = random_hex(bytes);
    let got = fsutil::create_once(path, fresh.as_bytes()).ok()?;
    let s = String::from_utf8(got).ok()?.trim().to_string();
    valid_hex(&s, bytes * 2).then_some(s)
}

impl Identity {
    /// Loads or creates the identity under `dir` (`$CUA_HOME/telemetry`).
    /// Falls back to a process-only identity when the directory is not
    /// writable.
    pub fn load_or_create(dir: &Path) -> Self {
        let id = load_or_create(&dir.join(ID_FILE), 16);
        let salt = load_or_create(&dir.join(SALT_FILE), 32);
        match (id, salt) {
            (Some(id), Some(salt)) => {
                let mut s = [0u8; 32];
                if hex::decode_to_slice(&salt, &mut s).is_ok() {
                    return Self {
                        id,
                        salt: s,
                        persisted: true,
                    };
                }
                Self::ephemeral()
            }
            _ => Self::ephemeral(),
        }
    }

    /// A process-only identity.
    pub fn ephemeral() -> Self {
        let mut salt = [0u8; 32];
        rand::rng().fill_bytes(&mut salt);
        Self {
            id: random_hex(16),
            salt,
            persisted: false,
        }
    }

    /// The stored install id, if any (never creates one).
    pub fn peek(dir: &Path) -> Option<String> {
        std::fs::read_to_string(dir.join(ID_FILE))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| valid_hex(s, 32))
    }

    /// A per-install salted hash of `value` in namespace `kind`: 16 hex.
    /// Stable on this install, meaningless anywhere else, not reversible.
    pub fn hash(&self, kind: &str, value: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.salt);
        h.update([0]);
        h.update(kind.as_bytes());
        h.update([0]);
        h.update(value.as_bytes());
        hex::encode(h.finalize())[..16].to_string()
    }
}

/// Deletes the install id and salt under `dir`.
pub fn reset(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for f in [ID_FILE, SALT_FILE] {
        let p = dir.join(f);
        if p.exists() {
            fsutil::remove(&p)?;
            removed.push(p);
        }
    }
    Ok(removed)
}

/// The first 8 characters, for `status`.
pub fn redact(id: &str) -> String {
    format!("{}…", id.chars().take(8).collect::<String>())
}
