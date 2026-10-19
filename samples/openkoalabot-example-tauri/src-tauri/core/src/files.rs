// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Attachments: drop a host file into the Space's `~/Downloads[/subdir]`.
//! The runtime verifies every file's SHA-256 in the guest; [`send_verified`]
//! also compares against the host's own hash, so "delivered" means the same
//! bytes, not just "no error".

use crate::{Error, Result};
use cua_spaces::Space;
use cua_spaces::files::{SendFileOptions, SendFileReport};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// SHA-256 of a local file, streamed (never whole in memory).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// The scenario's deterministic fixture bytes: xorshift64
/// (`x ^= x<<13; x ^= x>>7; x ^= x<<17`, wrapping u64), one byte (`x & 0xff`)
/// per step. Every implementation generates the same bytes from the seed.
pub fn xorshift_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// What the UI shows after a drop.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SentSummary {
    pub guest_path: String,
    pub bytes: u64,
    pub sha256: String,
    pub verified: bool,
}

/// Sends `path` and checks that the guest's SHA-256 matches this host's.
pub async fn send_verified(space: &Space, path: &Path, subdir: &str) -> Result<SentSummary> {
    let local = if path.is_file() {
        Some(sha256_file(path)?)
    } else {
        None
    };
    // #region docs:rs-send-file
    let report: SendFileReport = space
        .send_file(
            path,
            SendFileOptions {
                subdir: subdir.to_string(),
                ..Default::default()
            },
        )
        .await?;
    // #endregion docs:rs-send-file
    if !report.verified {
        return Err(Error::Invalid(format!(
            "the Space did not verify {}",
            report.dest
        )));
    }
    let first = report.files.first();
    if let (Some(local), Some(f)) = (&local, first)
        && f.sha256 != *local
    {
        return Err(Error::Invalid(format!(
            "guest sha256 {} != host {local}",
            f.sha256
        )));
    }
    Ok(SentSummary {
        guest_path: first.map(|f| f.path.clone()).unwrap_or(report.dest.clone()),
        bytes: report.bytes,
        sha256: local.unwrap_or_default(),
        verified: report.verified,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_bytes_match_the_scenario_hash() {
        // samples/openkoalabot-example-scenario/scenario.json `file.sha256`.
        let b = xorshift_bytes(1 << 20, 0x9E37_79B9_7F4A_7C15);
        assert_eq!(
            hex::encode(Sha256::digest(&b)),
            "c7f83242f7967cc3ff4cacc879c31c6fde4596b95b1820a9c061e83df73d0d6f"
        );
    }

    #[test]
    fn sha256_file_streams() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
