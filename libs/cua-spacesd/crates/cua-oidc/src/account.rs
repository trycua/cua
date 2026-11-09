// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The issuer's account id (`acct`): the path segment of the per-account
//! issuer `https://oidc.cua.ai/a/{acct}` and the `sub` claim
//! `account:{acct}`.
//!
//! Derivation: `acct = lowercase(base32(HMAC-SHA256(secret,
//! "cua-oidc-account/v1\n" || account)))[..12]`, where `account` is the
//! Keycloak account claim (`sub` by default) and `secret` is the issuer's
//! server-side account secret (32 bytes or more, from a file).
//!
//! - **Stable**: the same account always gets the same id, for as long as
//!   the secret is kept. Rotating the secret changes every id, which breaks
//!   every customer's trust setup, so the secret is never rotated.
//! - **Not reversible**: without the secret nobody can tell which Keycloak
//!   account an id belongs to, or compute the id of a known account.
//! - **Opaque and short**: 12 characters of RFC 4648 base32 (`a-z`, `2-7`),
//!   60 bits, so a collision is negligible at any realistic account count.
//!   Staging and prod use different secrets (and different Keycloak realms),
//!   so their ids differ.

use hmac::{Hmac, Mac as _};
use sha2::Sha256;

/// Length of an account id.
pub const ACCOUNT_ID_LEN: usize = 12;
/// Shortest accepted account secret.
pub const MIN_SECRET_LEN: usize = 32;

/// Derives account ids from Keycloak accounts.
#[derive(Clone)]
pub struct AccountIds {
    secret: Vec<u8>,
}

impl std::fmt::Debug for AccountIds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountIds(..)")
    }
}

impl AccountIds {
    /// With `secret` (at least [`MIN_SECRET_LEN`] bytes).
    pub fn new(secret: &[u8]) -> Result<Self, String> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(format!(
                "the account secret must be at least {MIN_SECRET_LEN} bytes"
            ));
        }
        Ok(Self {
            secret: secret.to_vec(),
        })
    }

    /// The account id of Keycloak account `account`.
    pub fn derive(&self, account: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("any key length");
        mac.update(b"cua-oidc-account/v1\n");
        mac.update(account.as_bytes());
        let digest = mac.finalize().into_bytes();
        let mut id = data_encoding::BASE32_NOPAD.encode(&digest);
        id.make_ascii_lowercase();
        id.truncate(ACCOUNT_ID_LEN);
        id
    }
}

/// Whether `id` has the shape of an account id.
pub fn valid_account_id(id: &str) -> bool {
    id.len() == ACCOUNT_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_short_and_keyed() {
        let ids = AccountIds::new(&[7u8; 32]).unwrap();
        let a = ids.derive("f1d2c3b4-0000-4000-8000-000000000001");
        assert_eq!(a, ids.derive("f1d2c3b4-0000-4000-8000-000000000001"));
        assert!(valid_account_id(&a), "{a}");
        assert_ne!(a, ids.derive("f1d2c3b4-0000-4000-8000-000000000002"));
        // Another secret (another environment): another id.
        let other = AccountIds::new(&[8u8; 32]).unwrap();
        assert_ne!(a, other.derive("f1d2c3b4-0000-4000-8000-000000000001"));
        assert!(AccountIds::new(b"short").is_err());
    }

    #[test]
    fn account_id_shape() {
        assert!(valid_account_id("abcdefgh2345"));
        assert!(!valid_account_id("abcdefgh234"));
        assert!(!valid_account_id("ABCDEFGH2345"));
        assert!(!valid_account_id("abcdefgh2341"));
        assert!(!valid_account_id("abcdefgh/../"));
    }
}
