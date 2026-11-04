//! Provider API keys and the Cua identity each one maps to.
//!
//! `keys.json` (0600) under the provider's directory:
//!
//! ```json
//! {"keys": [{"key": "e2b_…", "label": "default"},
//!           {"key": "e2b_…", "label": "ci", "daemon": "http://127.0.0.1:7777", "token": "…"}]}
//! ```
//!
//! A key without `daemon`/`token` uses the ambient Cua identity (the
//! running daemon, else an embedded runtime). A key with them uses that
//! daemon and token: one gateway can front several Cua identities. Each
//! key only sees the sandboxes it created.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyEntry {
    pub key: String,
    #[serde(default)]
    pub label: String,
    /// Cua daemon address for this key (socket path or loopback URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<String>,
    /// Cua daemon token for this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl KeyEntry {
    /// A short, non-secret id for records and logs.
    pub fn id(&self) -> String {
        use sha2::Digest;
        hex::encode(&sha2::Sha256::digest(self.key.as_bytes())[..6])
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Keys {
    pub keys: Vec<KeyEntry>,
}

impl Keys {
    /// Loads `path`, creating it with one new key (`<prefix><40 hex>`) when
    /// it does not exist.
    pub fn load_or_create(path: &Path, prefix: &str) -> std::io::Result<Keys> {
        if let Ok(bytes) = std::fs::read(path) {
            let keys: Keys = serde_json::from_slice(&bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?;
            if !keys.keys.is_empty() {
                return Ok(keys);
            }
        }
        let keys = Keys {
            keys: vec![KeyEntry {
                key: new_key(prefix),
                label: "default".into(),
                daemon: None,
                token: None,
            }],
        };
        keys.save(path)?;
        Ok(keys)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::tls::write_file(path, &serde_json::to_vec_pretty(self).expect("keys"), 0o600)
    }

    /// The entry for a presented key (constant-time comparison).
    pub fn find(&self, presented: &str) -> Option<&KeyEntry> {
        let mut found = None;
        for k in &self.keys {
            if bool::from(k.key.as_bytes().ct_eq(presented.as_bytes())) {
                found = Some(k);
            }
        }
        found
    }
}

/// `<prefix><40 lowercase hex>` (E2B's key format is `e2b_<hex>`).
pub fn new_key(prefix: &str) -> String {
    use rand::RngCore;
    let mut b = [0u8; 20];
    rand::rng().fill_bytes(&mut b);
    format!("{prefix}{}", hex::encode(b))
}

/// A random URL-safe token (envd access tokens, traffic tokens).
pub fn new_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Constant-time string equality.
pub fn same(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// The provider directory under the Cua home: `$CUA_HOME/compat/<name>`.
pub fn provider_dir(name: &str) -> PathBuf {
    cua_home::cua_home().join("compat").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_one_key_in_e2b_format_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("e2b/keys.json");
        let keys = Keys::load_or_create(&p, "e2b_").unwrap();
        assert_eq!(keys.keys.len(), 1);
        let k = &keys.keys[0].key;
        assert!(k.starts_with("e2b_") && k.len() == 44);
        assert!(k[4..].chars().all(|c| c.is_ascii_hexdigit()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Stable across loads.
        let again = Keys::load_or_create(&p, "e2b_").unwrap();
        assert_eq!(again.keys, keys.keys);
        assert!(again.find(k).is_some());
        assert!(again.find("e2b_0000").is_none());
    }
}
