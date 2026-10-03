// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Resolves a Chromium-family browser's "Safe Storage" key(s) on this host,
//! lazily and at most once each, through the injected [`HostEffects`].
//!
//! [`crate::passwords`] (saved passwords) and [`crate::cookies`] (cookies)
//! both decrypt values from the same browser profile with the exact same
//! per-machine key, so this is the one place that resolves it. The actual
//! cipher lives in [`cua_teleport_bundle::chromium_crypto`] (shared with the
//! receiver, which uses it to re-encrypt for the destination's own key --
//! never this one).

use cua_teleport_bundle::chromium_crypto;
use zeroize::{Zeroize, Zeroizing};

use crate::host::{EffectKind, HostCommand, HostEffects};
use crate::{Platform, TeleportError};

/// The `Local State` file next to a profile: inside it (an Electron app's
/// userData) or in its parent (a browser's `User Data` above `Default`).
pub fn default_local_state(profile_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let inside = profile_dir.join("Local State");
    if inside.is_file() {
        return Some(inside);
    }
    let above = profile_dir.parent()?.join("Local State");
    above.is_file().then_some(above)
}

/// Lazily resolves and caches a Chromium-family browser's Safe Storage
/// key(s) on this host. One instance per (browser, profile) read.
pub struct SafeStorageKeys<'a> {
    host: &'a dyn HostEffects,
    platform: Platform,
    /// The macOS Keychain service name for this browser ("Chrome Safe
    /// Storage", "Brave Safe Storage", …); see
    /// [`chromium_crypto::macos_safe_storage_service`].
    macos_service: &'static str,
    v10: Option<Zeroizing<[u8; 16]>>,
    v11: Option<Zeroizing<[u8; 16]>>,
    /// Windows: the browser's `Local State` and the DPAPI that unwraps its
    /// key (`v10` is AES-256-GCM under it).
    local_state: Option<std::path::PathBuf>,
    dpapi: std::sync::Arc<dyn cua_chromium_storage::dpapi::Dpapi>,
    windows_key: Option<Zeroizing<[u8; 32]>>,
}

impl<'a> SafeStorageKeys<'a> {
    /// A resolver for `macos_service` (e.g. `"Chrome Safe Storage"`) on
    /// `platform`, reading through `host`.
    pub fn new(host: &'a dyn HostEffects, platform: Platform, macos_service: &'static str) -> Self {
        Self {
            host,
            platform,
            macos_service,
            v10: None,
            v11: None,
            local_state: None,
            dpapi: std::sync::Arc::new(cua_chromium_storage::dpapi::SystemDpapi),
            windows_key: None,
        }
    }

    /// The `Local State` file whose `os_crypt.encrypted_key` holds this
    /// browser's Windows key.
    pub fn with_local_state(mut self, path: Option<std::path::PathBuf>) -> Self {
        self.local_state = path;
        self
    }

    /// DPAPI to unwrap the Windows key with (the system's by default).
    pub fn with_dpapi(
        mut self,
        dpapi: std::sync::Arc<dyn cua_chromium_storage::dpapi::Dpapi>,
    ) -> Self {
        self.dpapi = dpapi;
        self
    }

    /// The Windows AES-256-GCM key (read once from `Local State`).
    fn windows_key(&mut self) -> Result<&[u8; 32], TeleportError> {
        if self.windows_key.is_none() {
            let path = self.local_state.clone().ok_or_else(|| {
                TeleportError::Provider(
                    "this browser's Local State was not found, so its Windows key cannot be read"
                        .into(),
                )
            })?;
            let key = cua_chromium_storage::dpapi::local_state_key(&path, self.dpapi.as_ref())
                .map_err(|e| TeleportError::Provider(format!("Windows browser key: {e}")))?;
            self.windows_key = Some(key);
        }
        Ok(self.windows_key.as_deref().expect("set above"))
    }

    /// The `v10` key: the macOS Keychain secret (prompting for
    /// authorization the first time, like any other Keychain read) or
    /// Linux's fixed `peanuts` password.
    pub fn v10(&mut self) -> Result<&[u8; 16], TeleportError> {
        if self.v10.is_none() {
            let key = match self.platform {
                Platform::Linux => chromium_crypto::derive_key(
                    chromium_crypto::LINUX_V10_PASSWORD,
                    chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
                ),
                Platform::MacOS => {
                    let item = crate::keychain::read_generic(self.host, self.macos_service, None)
                        .ok_or_else(|| {
                        TeleportError::Provider(format!(
                            "the {} Keychain item could not be read, so this browser's \
                                 Safe-Storage-encrypted values cannot be decrypted",
                            self.macos_service
                        ))
                    })?;
                    let mut secret = item.secret.clone();
                    let key =
                        chromium_crypto::derive_key(&secret, chromium_crypto::MACOS_PBKDF2_ROUNDS);
                    secret.zeroize();
                    key
                }
                Platform::Windows => {
                    return Err(TeleportError::Provider(
                        "Windows browsers use AES-GCM, not a v10 CBC key".into(),
                    ));
                }
            };
            self.v10 = Some(key);
        }
        Ok(self.v10.as_ref().expect("set above"))
    }

    /// The `v11` key (Linux libsecret only).
    pub fn v11(&mut self) -> Result<&[u8; 16], TeleportError> {
        if self.platform != Platform::Linux {
            return Err(TeleportError::Provider(
                "a v11 Safe-Storage-encrypted value outside Linux is not a Chromium format".into(),
            ));
        }
        if self.v11.is_none() {
            // Chromium's libsecret schema stores one secret per application,
            // labeled "chrome" regardless of the channel/brand.
            let cmd = HostCommand::new(EffectKind::KeychainRead, "secret-tool")
                .args(["lookup", "application", "chrome"])
                .timeout(std::time::Duration::from_secs(20));
            let out = self.host.run(&cmd).map_err(|e| {
                TeleportError::Provider(format!(
                    "reading the libsecret Safe Storage key failed: {e}"
                ))
            })?;
            if !out.success || out.stdout.is_empty() {
                return Err(TeleportError::Provider(
                    "the libsecret Safe Storage key is not available, so v11 values cannot be \
                     decrypted"
                        .into(),
                ));
            }
            let mut secret = out.stdout;
            while secret.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                secret.pop();
            }
            let key =
                chromium_crypto::derive_key(&secret, chromium_crypto::LINUX_V11_PBKDF2_ROUNDS);
            secret.zeroize();
            self.v11 = Some(key);
        }
        Ok(self.v11.as_ref().expect("set above"))
    }

    /// Decrypts one `v10`/`v11`-prefixed value (`Login Data`'s
    /// `password_value` or `Cookies`' `encrypted_value`), resolving whichever
    /// key its prefix names.
    pub fn decrypt(&mut self, value: &[u8]) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
        if self.platform == Platform::Windows {
            // `v10` is AES-256-GCM under the DPAPI-wrapped Local State key.
            // `v20` (app-bound) is refused by callers before they get here.
            if value.starts_with(b"v20") {
                return Err(app_bound_unsupported());
            }
            let key = *self.windows_key()?;
            return chromium_crypto::gcm::decrypt(&key, value);
        }
        match value.get(..3) {
            Some(b"v10") => {
                let key = *self.v10()?;
                chromium_crypto::decrypt(&key, &value[3..])
            }
            Some(b"v11") => {
                let key = *self.v11()?;
                chromium_crypto::decrypt(&key, &value[3..])
            }
            Some(b"v20") => Err(app_bound_unsupported()),
            _ => Err(TeleportError::Provider(
                "a Safe-Storage-encrypted value uses an encryption version this build does not \
                 know"
                    .into(),
            )),
        }
    }
}

/// Chrome 127+ on Windows encrypts new cookies with App-Bound Encryption
/// (`v20`): the key is bound to Chrome's own elevation service, so no other
/// program, this one included, can decrypt the values.
fn app_bound_unsupported() -> TeleportError {
    TeleportError::Provider(
        "these cookies are protected by Chrome's App-Bound Encryption (v20, Windows Chrome \
         127+), which only Chrome itself can decrypt, so they cannot be teleported. Sign in \
         again in the destination browser instead."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{FakeHost, HostOutput};
    use std::sync::Arc;

    #[test]
    fn v20_values_get_a_clear_app_bound_message_on_every_platform() {
        for platform in [Platform::Windows, Platform::MacOS, Platform::Linux] {
            let host = FakeHost::new();
            let mut keys = SafeStorageKeys::new(&host, platform, "Chrome Safe Storage");
            let err = keys.decrypt(b"v20\x00\x01\x02").unwrap_err().to_string();
            assert!(err.contains("App-Bound Encryption"), "{err}");
            assert!(err.contains("Sign in again"), "{err}");
        }
    }

    #[test]
    fn macos_v10_reads_the_named_service_through_the_host() {
        let host = Arc::new(FakeHost::new().with_responder(|c| {
            if c.args.iter().any(|a| a == "-w") {
                Ok(HostOutput::ok(b"a-safe-storage-secret\n".to_vec()))
            } else {
                Ok(HostOutput::ok(
                    b"    \"acct\"<blob>=\"Brave\"\n    \"svce\"<blob>=\"Brave Safe Storage\"\n"
                        .to_vec(),
                ))
            }
        }));
        let mut keys = SafeStorageKeys::new(host.as_ref(), Platform::MacOS, "Brave Safe Storage");
        if cfg!(target_os = "macos") {
            let want = chromium_crypto::derive_key(
                b"a-safe-storage-secret",
                chromium_crypto::MACOS_PBKDF2_ROUNDS,
            );
            assert_eq!(keys.v10().unwrap(), &*want);
        } else {
            assert!(keys.v10().is_err());
        }
    }

    #[test]
    fn linux_v10_needs_no_host_read() {
        let host = FakeHost::new();
        let mut keys = SafeStorageKeys::new(&host, Platform::Linux, "Chrome Safe Storage");
        let want = chromium_crypto::derive_key(
            chromium_crypto::LINUX_V10_PASSWORD,
            chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        );
        assert_eq!(keys.v10().unwrap(), &*want);
        assert!(host.calls().is_empty());
    }

    #[test]
    fn decrypt_dispatches_on_the_value_prefix() {
        let host = FakeHost::new();
        let mut keys = SafeStorageKeys::new(&host, Platform::Linux, "Chrome Safe Storage");
        let key = chromium_crypto::derive_key(
            chromium_crypto::LINUX_V10_PASSWORD,
            chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        );
        let ct = chromium_crypto::encrypt_v10(&key, b"cookie-value");
        assert_eq!(&*keys.decrypt(&ct).unwrap(), b"cookie-value");
        assert!(keys.decrypt(b"unk-not-a-known-prefix").is_err());
    }

    #[test]
    fn windows_has_no_cbc_keys_and_decrypts_v10_gcm_through_local_state() {
        use cua_chromium_storage::dpapi::{Dpapi, FakeDpapi, ensure_local_state_key};
        let host = FakeHost::new();
        let mut keys = SafeStorageKeys::new(&host, Platform::Windows, "Chrome Safe Storage");
        assert!(keys.v10().unwrap_err().to_string().contains("Windows"));
        assert!(keys.v11().is_err());
        // Without a Local State there is nothing to decrypt with.
        assert!(
            keys.decrypt(b"v10-anything-long-enough-for-a-nonce-and-tag")
                .is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let ls = dir.path().join("Local State");
        let (key, _) = ensure_local_state_key(&ls, &FakeDpapi, [5u8; 32]).unwrap();
        let enc = chromium_crypto::gcm::encrypt(&key, &[1u8; 12], b"win-cookie");
        let dpapi: std::sync::Arc<dyn Dpapi> = std::sync::Arc::new(FakeDpapi);
        let mut keys = SafeStorageKeys::new(&host, Platform::Windows, "Chrome Safe Storage")
            .with_local_state(Some(ls))
            .with_dpapi(dpapi);
        assert_eq!(&*keys.decrypt(&enc).unwrap(), b"win-cookie");
        // App-bound values are never attempted.
        let mut v20 = b"v20".to_vec();
        v20.extend_from_slice(&[0u8; 40]);
        assert!(
            keys.decrypt(&v20)
                .unwrap_err()
                .to_string()
                .contains("App-Bound")
        );
    }
}
