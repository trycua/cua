// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What vouches for the permission settings file (`approvals.json`).
//!
//! The file carries an HMAC (see `cua_spaces::approvals`); the key and the
//! newest generation live in a dedicated Keychain item per cua home, whose
//! access list names the signed Cua Spaces app and `cua`. Those read it
//! without a prompt; another program of the same user cannot read it, and
//! so cannot make a file that verifies. Reads never show the user a prompt:
//! when the item cannot be read the settings fall back to "everything asks".
//! Other platforms have no seal yet, so the settings cannot be changed there.

use cua_spaces::approvals::PolicySeal;
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod mac {
    use cua_keyvault::macos::AclSecret;
    use cua_spaces::approvals::{PolicySeal, SealState};
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};

    const SERVICE: &str = "Cua Permissions Seal";

    /// The Keychain-backed seal.
    pub struct KeychainSeal {
        secret: AclSecret,
        trusted: Vec<PathBuf>,
    }

    impl KeychainSeal {
        /// `keychain`: `None` is the user's default keychain; a path is a
        /// throwaway one (tests).
        pub fn new(keychain: Option<PathBuf>) -> Self {
            // The other programs that read it: the `cua` this one ships
            // beside, and the apps.
            let mut trusted = vec![];
            if let Some(exe) = std::env::current_exe().ok() {
                trusted.extend(cua_home::bundled_cua_of(&exe));
            }
            for app in [
                "/Applications/Cua Spaces.app/Contents/MacOS/Cua Spaces",
                "/Applications/Cua Spaces.app/Contents/MacOS/cua",
            ] {
                trusted.push(PathBuf::from(app));
            }
            Self {
                secret: AclSecret::new(SERVICE, keychain),
                trusted,
            }
        }

        fn account(home: &Path) -> String {
            let h = Sha256::digest(home.to_string_lossy().as_bytes());
            format!("approvals-{}", hex::encode(&h[..8]))
        }
    }

    impl PolicySeal for KeychainSeal {
        fn load(&self, home: &Path) -> Result<Option<SealState>, String> {
            let Some(bytes) = self
                .secret
                .read(&Self::account(home))
                .map_err(|e| e.to_string())?
            else {
                return Ok(None);
            };
            if bytes.len() != 40 {
                return Err("the sealed secret is damaged".into());
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes[..32]);
            let generation = u64::from_be_bytes(bytes[32..].try_into().unwrap());
            Ok(Some(SealState { key, generation }))
        }

        fn save(&self, home: &Path, state: &SealState) -> Result<(), String> {
            let mut bytes = state.key.to_vec();
            bytes.extend(state.generation.to_be_bytes());
            self.secret
                .write(&Self::account(home), &bytes, &self.trusted)
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::KeychainSeal;

/// This platform's seal, if it has one.
pub fn platform_seal() -> Option<Arc<dyn PolicySeal>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(KeychainSeal::new(None)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Registers this platform's seal for the process (idempotent). Called by
/// the Cua Spaces `cua` and the app's core at startup.
pub fn register_platform_seal() {
    if let Some(seal) = platform_seal() {
        cua_spaces::approvals::register_seal(seal);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use cua_spaces::approvals::{Approver, Cap, Policy};

    struct Yes;
    impl Approver for Yes {
        fn confirm(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    // Creates a keychain file, which can make macOS show prompts: run it on
    // purpose with `--ignored`.
    #[test]
    #[ignore = "creates a throwaway keychain; macOS may prompt"]
    fn the_keychain_seal_makes_a_hand_edit_ineffective() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("throwaway.keychain");
        cua_keyvault::macos::create_throwaway_keychain(&kc).unwrap();
        let seal = KeychainSeal::new(Some(kc));
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let s: Option<&dyn PolicySeal> = Some(&seal);
        Policy::set_with(&home, Cap::Cloud, false, &Yes, s).unwrap();
        let l = Policy::load_with(&home, s);
        assert!(l.notice.is_none() && !l.policy.requires(Cap::Cloud));
        // A same-user process loosens another row by hand.
        let p = cua_spaces::approvals::path_in(&home);
        let t = std::fs::read_to_string(&p).unwrap();
        std::fs::write(
            &p,
            t.replace("\"host_files\": true", "\"host_files\": false"),
        )
        .unwrap();
        let l = Policy::load_with(&home, s);
        assert_eq!(l.policy, Policy::strict());
        assert!(l.notice.is_some());
    }
}
