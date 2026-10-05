// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Keyvault (the "Cua locker"): the one place teleport secrets
//! live on a user's machine, and the only broker that moves them.
//!
//! Teleport goes **source app → Keyvault → target Space**. Cua imports
//! sessions per site or per app into an encrypted vault; targets receive
//! them only from the vault; third-party apps that embed the cua SDK ask the
//! vault (over a local socket whose peers are verified by the kernel and
//! their code signature) and the user decides in Cua's consent UI.
//!
//! - [`crypto`]: the envelope (ring AEAD, HKDF, HMAC; RustCrypto Argon2id).
//! - [`protector`]: independent protectors that each wrap the vault master
//!   key (macOS Keychain, Windows Credential Manager, passphrase, recovery
//!   key).
//! - [`store`]: the on-disk vault (sealed metadata, per-item keys).
//! - [`audit`]: the hash-chained, MAC'd audit log.
//! - [`capability`]: macaroon-style capability tokens.
//! - [`caller`]: verified peer identities (audit token + code signature).
//! - [`policy`]: grants, unattended rules, the kill switch.
//! - [`broker`]: the service logic behind every IPC operation.
//! - [`ipc`]: the socket protocol, server and client.
//! - [`embedded`]: the SDK side: `RequiresCuaApp` and the (compiled-out)
//!   enterprise embedded seam.
//!
//! Design and threat model: `docs/content/docs/spaces/guides/keyvault.mdx`
//! and the security-model page.

pub mod audit;
pub mod broker;
pub mod caller;
/// The Keyvault IPC client, as the SDK, the CLI and the daemon-hosted MCP
/// tools reach it: `cua_keyvault::client::KeyvaultClient`. It is the same
/// client as [`ipc::KeyvaultClient`], re-exported under the name the design
/// (sections 5.11, 5.13) and callers use.
pub mod client {
    pub use crate::ipc::{ConnectError, KeyvaultClient, ServerCheck};
}
pub mod capability;
pub mod confusable;
pub mod crypto;
pub mod embedded;
pub mod ipc;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod model;
pub mod policy;
pub mod protector;
pub mod record;
pub mod rollback;
pub mod store;
/// Broker-side usage telemetry (counts and fixed vocabularies only).
pub mod telemetry;

pub use broker::{Backend, Broker, BrokerConfig, UserPresence};
pub use caller::{CallerIdentity, Signing, TrustPolicy};
pub use capability::{Action, Capability, Caveat};
pub use model::{ItemKind, ItemMeta, ItemPayload, ItemPolicy, Meta};
pub use store::Vault;
/// Secrets the Keyvault takes (passphrases) travel as [`Zeroizing`] strings,
/// so callers wipe their copy the way the vault wipes its own.
pub use zeroize::Zeroizing;

/// Every keyvault failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A cryptographic check failed. Deliberately vague.
    #[error("crypto: {0}")]
    Crypto(String),
    /// Wrong passphrase, recovery key or OS secret.
    #[error("the credential does not unlock this vault")]
    WrongCredential,
    /// The vault is locked.
    #[error("the Keyvault is locked")]
    Locked,
    /// No vault exists.
    #[error("no Keyvault at {0}; run `cua keyvault init`")]
    NoVault(String),
    /// The global kill switch is on.
    #[error("the Keyvault is disabled (global kill switch); enable it in Cua to teleport")]
    Disabled,
    /// The vault was written by an earlier preview build (format 1, whole-app
    /// items). It is not migrated.
    #[error(
        "the Keyvault was created by an earlier preview (format {0}) and cannot be read; it was set aside"
    )]
    OldFormat(u32),
    /// A file failed an integrity check.
    #[error("the Keyvault is damaged: {0}")]
    Corrupt(String),
    /// Bad input.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// A capability token failed verification.
    #[error("capability refused: {0}")]
    Capability(String),
    /// The caller may not do this.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// User presence (Touch ID / password) is required and was not given.
    #[error("user presence was not confirmed: {0}")]
    PresenceFailed(String),
    /// The user declined.
    #[error("denied: {0}")]
    Denied(String),
    /// The OS key store failed.
    #[error("os key store: {0}")]
    Os(String),
    /// Not available on this platform or build.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Refused in tests.
    #[error("host effects refused: {0}")]
    HostEffectsRefused(String),
    /// Too many requests.
    #[error("rate limited: {0}")]
    RateLimited(String),
    /// A backend (capture or delivery) failed.
    #[error("{0}")]
    Backend(String),
    /// I/O.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Now, Unix milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The environment switch every Cua crate honours to refuse real-machine
/// effects in tests (the login keychain, OS prompts).
pub const TEST_SANDBOX_ENV: &str = "CUA_ENV_TEST_SANDBOX";

/// Whether real-machine effects are refused: in this crate's unit tests and
/// whenever `CUA_ENV_TEST_SANDBOX=1`.
pub fn host_effects_forbidden() -> bool {
    cfg!(test)
        || std::env::var(TEST_SANDBOX_ENV)
            .map(|v| v == "1")
            .unwrap_or(false)
}

/// Where the vault lives: `$CUA_HOME/keyvault` (default `~/.cua/keyvault`).
pub fn default_dir() -> Option<std::path::PathBuf> {
    cua_home().map(|h| h.join("keyvault"))
}

/// The keyvault socket: `$CUA_HOME/keyvault.sock`.
pub fn default_socket() -> Option<std::path::PathBuf> {
    cua_home().map(|h| h.join("keyvault.sock"))
}

fn cua_home() -> Option<std::path::PathBuf> {
    if let Some(h) = std::env::var_os("CUA_HOME").filter(|v| !v.is_empty()) {
        if !cua_home_within_real_home() {
            // A malicious embedder can set $CUA_HOME to point the SDK (and its
            // socket lookup) at a directory it controls (red-team F16). We do
            // not silently trust an override outside the user's real home.
            tracing::warn!(
                cua_home = ?h,
                "CUA_HOME points outside the user's home; the Keyvault socket there is not trusted"
            );
        }
        return Some(h.into());
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|h| std::path::PathBuf::from(h).join(".cua"))
}

/// Whether `$CUA_HOME`, if set, resolves within the user's real home. A
/// malicious embedder can set `$CUA_HOME` to steer the SDK at a directory it
/// controls; callers warn or refuse when this is false (red-team F16). An unset
/// `$CUA_HOME` (the default `~/.cua`) is trusted.
pub fn cua_home_within_real_home() -> bool {
    let Some(cua) = std::env::var_os("CUA_HOME").filter(|v| !v.is_empty()) else {
        return true;
    };
    let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) else {
        return false;
    };
    let cua = std::path::PathBuf::from(&cua);
    let home = std::path::PathBuf::from(&home);
    let cua = std::fs::canonicalize(&cua).unwrap_or(cua);
    let home = std::fs::canonicalize(&home).unwrap_or(home);
    cua.starts_with(&home)
}

/// Validates that a directory meant to hold the Keyvault socket is safe: it is
/// a directory this user owns and that no other user can enter (mode 0700, no
/// group/other bits). This is what protects a Linux client, where the server's
/// code signature is unverifiable, from a fake daemon planted in a
/// caller-controlled `$CUA_HOME` (red-team F16). It also stops the daemon from
/// binding inside a directory an attacker could reach.
#[cfg(unix)]
pub fn validate_socket_dir(dir: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let md = std::fs::symlink_metadata(dir)
        .map_err(|e| Error::Invalid(format!("{}: {e}", dir.display())))?;
    if !md.file_type().is_dir() {
        return Err(Error::Forbidden(format!(
            "{} is not a real directory (a symlink or file could redirect the socket)",
            dir.display()
        )));
    }
    // SAFETY: getuid has no preconditions.
    let ours = unsafe { libc::getuid() };
    if md.uid() != ours {
        return Err(Error::Forbidden(format!(
            "{} is owned by uid {}, not this user; refusing",
            dir.display(),
            md.uid()
        )));
    }
    let mode = md.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(Error::Forbidden(format!(
            "{} is reachable by other users (mode {mode:o}); the Keyvault socket must live in a 0700 dir",
            dir.display()
        )));
    }
    Ok(())
}
