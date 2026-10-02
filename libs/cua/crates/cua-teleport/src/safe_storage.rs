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
        }
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
                Platform::Windows => return Err(windows_unsupported()),
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
            _ if self.platform == Platform::Windows => Err(windows_unsupported()),
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

fn windows_unsupported() -> TeleportError {
    TeleportError::Provider(
        "decrypting Chromium's Safe-Storage-encrypted values on Windows (DPAPI) is not \
         supported yet"
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
    fn windows_is_refused_for_every_key() {
        let host = FakeHost::new();
        let mut keys = SafeStorageKeys::new(&host, Platform::Windows, "Chrome Safe Storage");
        assert!(keys.v10().unwrap_err().to_string().contains("Windows"));
        assert!(keys.v11().is_err());
    }
}
