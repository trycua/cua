// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The effect-free teleport contract.
//!
//! "Teleport" moves a desktop application's session (tabs, profile,
//! credentials) from the user's machine into a sandbox. It has two halves
//! that never link each other:
//!
//! - the **sender** (`cua-teleport`, in the cua SDK) runs on the user's
//!   machine: it describes, captures and uploads a session;
//! - the **receiver** (`cua-spacesd-teleport`, in cua-spacesd) runs inside the
//!   sandbox: it verifies, materializes and relaunches it.
//!
//! This crate is everything both halves must agree on, and nothing else:
//!
//! - [`bundle`]: the on-wire [`bundle::SessionBundle`] (a tar stream whose
//!   first entry is `manifest.json`, a [`bundle::BundleHeader`], followed by
//!   SHA-256-checked file entries), with [`bundle::BundleWriter`],
//!   [`bundle::BundleReader`] and the size limits;
//! - the shared data types ([`AppRef`], [`TransferScope`], [`ManifestItem`],
//!   [`TransferManifest`], [`LaunchSpec`], …);
//! - [`keychain`]: the reserved `keychain.json` entry format (items are read
//!   on the sender and installed on the receiver);
//! - [`local_storage`]: the reserved `localstorage.json` entry (a Chromium
//!   browser's localStorage as items);
//! - [`logins`]: the reserved `logins.json` entry (saved passwords the user
//!   ticked, re-encrypted by the receiver);
//! - [`layout`]: per-app layout descriptors — which profile files make up each
//!   app's session, the canonical bundle paths, and how they remap onto each
//!   destination platform -- as pure data and pure functions;
//! - [`chromium_crypto`]: the Chromium "Safe Storage" value cipher (cookies,
//!   saved passwords), used to decrypt on the sender and re-encrypt on the
//!   receiver with the receiver's own key -- never the sender's.
//!
//! It is **effect-free** by construction: no process, socket, filesystem,
//! environment or `$HOME` API appears in its source (a test in this crate
//! scans for them). Bundles are built from and read into memory; the halves
//! decide where bytes come from and where they land.

pub mod bundle;
pub mod chromium_crypto;
pub mod cookies;
mod error;
pub mod keychain;
pub mod layout;
pub mod local_storage;
pub mod logins;
mod types;

pub use error::{Result, TeleportError};
pub use types::{
    AppRef, InstallProbe, LaunchSpec, ManifestItem, Platform, TransferManifest, TransferScope,
    WindowRef, WindowRestore,
};

/// Bundle format version the sender produces and the receiver accepts
/// (`GetManifestResponse.bundle_version`).
pub const BUNDLE_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    /// Structural guard: this crate is the shared contract and must stay free
    /// of host effects, so it can never become a path to the real machine on
    /// either side. Any process, socket, filesystem, environment or `$HOME`
    /// API in its source fails this test.
    #[test]
    fn source_is_effect_free() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        let mut scanned = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                scanned += 1;
                // A Windows checkout may have CRLF line ends, which the
                // marker below must still match.
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("\r\n", "\n");
                // Everything after the `mod tests` marker of lib.rs is this
                // test itself, which legitimately reads the source tree.
                let text = match text.find(concat!("mod ", "tests {\n    /// Structural guard")) {
                    Some(end) => &text[..end],
                    None => &text[..],
                };
                for needle in [
                    "std::process",
                    "Command::new",
                    "std::net",
                    "TcpStream",
                    "UdpSocket",
                    "std::fs",
                    "File::",
                    "std::env",
                    "env::var",
                    "home_dir",
                    "std::thread",
                    "tokio",
                ] {
                    if text.contains(needle) {
                        offenders.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(scanned >= 5, "scanned only {scanned} files");
        assert!(
            offenders.is_empty(),
            "host effects in cua-teleport-bundle: {offenders:#?}"
        );
    }
}
