// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Keyed pseudonyms for account ids: the analytics (PostHog) distinct id and
//! the short tag log lines carry instead of the account id.
//!
//! Both are HMAC-SHA256 under a per-relay secret, so knowing an account id
//! (a Keycloak `sub`) does not tell you its analytics id or its log tag, and
//! two relays do not share them. The two are derived with different labels,
//! so one does not give the other away. The key is a 32-byte file next to
//! the state files (`<state>.analytics-key`, created 0600 on first use, or
//! `--analytics-key-file`); without a state file it lives in memory for the
//! process. Losing it only changes the ids: analytics continuity breaks once
//! and log tags stop matching older logs.
//!
//! The key is process-wide ([`install`]): log lines are written from places
//! with no relay handle. The first relay to install wins (one relay per
//! process in production); until one does, a random key is used.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn mac(label: &str, value: &str) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, current());
    let mut ctx = ring::hmac::Context::with_key(&key);
    ctx.update(label.as_bytes());
    ctx.update(&[0]);
    ctx.update(value.as_bytes());
    ctx.sign().as_ref().to_vec()
}

fn current() -> &'static [u8; 32] {
    KEY.get_or_init(random_key)
}

fn random_key() -> [u8; 32] {
    use rand::RngCore as _;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

/// The PostHog distinct id for account `sub` (`relay_` + 16 hex). Never the
/// account id or email, and not derivable from them without the relay's key.
pub fn distinct_id(sub: &str) -> String {
    format!("relay_{}", &hex::encode(mac("posthog", sub))[..16])
}

/// The tag log lines carry for account `sub` (8 hex): enough to follow one
/// account across lines, not reversible. Use this, never the account id or
/// an email, in anything logged.
pub fn tag(sub: &str) -> String {
    hex::encode(&mac("log", sub)[..4])
}

/// Installs the process-wide key read from (or created at) `path`; `None`
/// keeps the in-memory one. Returns whether this call set the key.
pub fn install(path: Option<&Path>) -> std::io::Result<bool> {
    let key = match path {
        Some(p) => load_or_create(p)?,
        None => return Ok(false),
    };
    Ok(KEY.set(key).is_ok())
}

/// Where the key lives by default: next to the state file.
pub fn default_path(state_file: Option<&Path>, devices_file: Option<&Path>) -> Option<PathBuf> {
    state_file
        .or(devices_file)
        .map(|p| PathBuf::from(format!("{}.analytics-key", p.display())))
}

/// Reads the 32-byte key at `path`, creating it (0600, exclusively, so two
/// relays starting together agree) when missing.
pub fn load_or_create(path: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Write as _;
    loop {
        match std::fs::read(path) {
            Ok(raw) => {
                return raw.try_into().map_err(|raw: Vec<u8>| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("{} is {} bytes, not 32", path.display(), raw.len()),
                    )
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let key = random_key();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(mut f) => {
                f.write_all(&key)?;
                f.sync_all()?;
                return Ok(key);
            }
            // Someone else created it between the read and now: read theirs.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_tags_are_keyed_stable_and_separate() {
        let a = distinct_id("sub-1");
        assert_eq!(a, distinct_id("sub-1"));
        assert_ne!(a, distinct_id("sub-2"));
        assert!(a.starts_with("relay_") && a.len() == 6 + 16);
        let t = tag("sub-1");
        assert_eq!(t.len(), 8);
        assert_eq!(t, tag("sub-1"));
        assert!(
            !a.contains(&t),
            "the tag is not a piece of the analytics id"
        );
        // Not the old unsalted hash of the sub.
        let old = ring::digest::digest(&ring::digest::SHA256, b"sub-1");
        assert_ne!(a, format!("relay_{}", &hex::encode(old.as_ref())[..16]));
    }

    #[test]
    fn the_key_file_is_created_0600_once_and_reread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json.analytics-key");
        let k1 = load_or_create(&path).unwrap();
        assert_eq!(k1, load_or_create(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap().len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::write(&path, b"short").unwrap();
        assert!(load_or_create(&path).is_err());
    }

    #[test]
    fn default_path_sits_next_to_the_state_files() {
        let p = default_path(Some(Path::new("/v/machines.json")), None).unwrap();
        assert_eq!(p, Path::new("/v/machines.json.analytics-key"));
        assert!(default_path(None, None).is_none());
    }
}
