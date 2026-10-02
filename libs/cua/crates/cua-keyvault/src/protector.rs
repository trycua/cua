// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Key protectors: independent ways to unwrap the vault master key (VMK),
//! such as a platform keystore, a passphrase or a recovery key.
//!
//! The VMK is random and never stored in the clear. Each protector wraps it
//! under its own key-encryption key (KEK) with AEAD, binding the vault id
//! and protector id as associated data:
//!
//! | Protector | KEK comes from | Unattended |
//! |-----------|----------------|------------|
//! | `macos-keychain` | a random 256-bit key in a macOS keychain item whose ACL trusts only the Cua binary that created it | yes, while the user is logged in |
//! | `windows-credential` | Windows Credential Manager (DPAPI, user scope) | yes |
//! | `passphrase` | Argon2id over the user's passphrase | no |
//! | `recovery` | a 160-bit random recovery key shown once | no |
//!
//! Linux has no OS protector yet: [`os_protector`] returns a typed
//! [`Error::Unsupported`] and the vault needs a passphrase.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams, Sealed, SecretKey};
use crate::{Error, Result};

/// Protector kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectorKind {
    /// A KEK in a macOS keychain item.
    MacosKeychain,
    /// A KEK in Windows Credential Manager.
    WindowsCredential,
    /// Argon2id over a passphrase.
    Passphrase,
    /// A random recovery key shown to the user once.
    Recovery,
}

impl ProtectorKind {
    /// True when unwrapping needs no user input (so unattended teleport can
    /// run while the user's OS session is unlocked).
    pub fn unattended(self) -> bool {
        matches!(
            self,
            ProtectorKind::MacosKeychain | ProtectorKind::WindowsCredential
        )
    }
}

/// A stored protector: how to find its KEK plus the wrapped VMK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectorRecord {
    /// Protector id (random).
    pub id: String,
    /// Kind.
    pub kind: ProtectorKind,
    /// Label for the UI.
    pub label: String,
    /// The VMK sealed under this protector's KEK.
    pub wrapped_vmk: Sealed,
    /// Argon2id parameters (`passphrase`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<KdfParams>,
    /// Keychain service name (`macos-keychain`, `windows-credential`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Keychain account name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Explicit keychain file (tests use a throwaway keychain); `None` is
    /// the user's default keychain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_path: Option<String>,
}

/// Associated data binding a wrapped VMK to its vault and protector.
fn vmk_aad(vault_id: &str, protector_id: &str) -> Vec<u8> {
    format!("cua-keyvault/v1/vmk|{vault_id}|{protector_id}").into_bytes()
}

/// A way to wrap and unwrap the VMK.
pub trait Protector: Send + Sync {
    /// Kind.
    fn kind(&self) -> ProtectorKind;

    /// Wraps `vmk` for a new protector record (may create an OS secret).
    fn enroll(&self, vault_id: &str, vmk: &SecretKey) -> Result<ProtectorRecord>;

    /// Unwraps the VMK from `record`.
    fn unwrap(&self, vault_id: &str, record: &ProtectorRecord) -> Result<SecretKey>;

    /// Removes any OS secret this protector created (best effort).
    fn remove(&self, _record: &ProtectorRecord) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn wrap_for(
    kind: ProtectorKind,
    label: &str,
    vault_id: &str,
    kek: &SecretKey,
    vmk: &SecretKey,
) -> Result<ProtectorRecord> {
    let id = crypto::random_id()?;
    let wrapped_vmk = crypto::seal(kek, &vmk_aad(vault_id, &id), vmk.expose())?;
    Ok(ProtectorRecord {
        id,
        kind,
        label: label.to_string(),
        wrapped_vmk,
        kdf: None,
        service: None,
        account: None,
        keychain_path: None,
    })
}

pub(crate) fn unwrap_for(
    kek: &SecretKey,
    vault_id: &str,
    record: &ProtectorRecord,
) -> Result<SecretKey> {
    let raw = crypto::open(kek, &vmk_aad(vault_id, &record.id), &record.wrapped_vmk)
        .map_err(|_| Error::WrongCredential)?;
    SecretKey::from_bytes(&raw)
}

/// The shortest passphrase a new passphrase protector accepts, in
/// characters. Unlocking never checks it, so a vault created under an older,
/// shorter minimum still opens.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

/// Argon2id passphrase protector.
pub struct PassphraseProtector {
    passphrase: Zeroizing<String>,
    params: Option<KdfParams>,
}

impl PassphraseProtector {
    /// Uses recommended Argon2id parameters when enrolling.
    pub fn new(passphrase: impl Into<String>) -> Self {
        Self {
            passphrase: Zeroizing::new(passphrase.into()),
            params: None,
        }
    }

    /// Enrolls with explicit parameters (tests use cheap ones).
    pub fn with_params(passphrase: impl Into<String>, params: KdfParams) -> Self {
        Self {
            passphrase: Zeroizing::new(passphrase.into()),
            params: Some(params),
        }
    }
}

impl Protector for PassphraseProtector {
    fn kind(&self) -> ProtectorKind {
        ProtectorKind::Passphrase
    }

    fn enroll(&self, vault_id: &str, vmk: &SecretKey) -> Result<ProtectorRecord> {
        check_new_passphrase(&self.passphrase)?;
        let params = match &self.params {
            Some(p) => p.clone(),
            None => KdfParams::recommended()?,
        };
        let kek = crypto::passphrase_key(self.passphrase.as_bytes(), &params)?;
        let mut rec = wrap_for(ProtectorKind::Passphrase, "Passphrase", vault_id, &kek, vmk)?;
        rec.kdf = Some(params);
        Ok(rec)
    }

    fn unwrap(&self, vault_id: &str, record: &ProtectorRecord) -> Result<SecretKey> {
        let params = record
            .kdf
            .as_ref()
            .ok_or_else(|| Error::Corrupt("passphrase protector without KDF parameters".into()))?;
        let kek = crypto::passphrase_key(self.passphrase.as_bytes(), params)?;
        unwrap_for(&kek, vault_id, record)
    }
}

/// Checks a passphrase for a new protector (length only; the apps and the
/// CLI add a strength hint). Never includes the passphrase in the error.
pub fn check_new_passphrase(passphrase: &str) -> Result<()> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(Error::Invalid(format!(
            "a Keyvault passphrase needs at least {MIN_PASSPHRASE_CHARS} characters"
        )));
    }
    Ok(())
}

/// A random recovery key, shown once as eight groups of five characters
/// (40 Crockford base32 characters encode 200 bits; 160 of them are key,
/// 40 are a checksum that catches typos).
#[derive(Clone)]
pub struct RecoveryKey(Zeroizing<String>);

impl std::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryKey(<redacted>)")
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl RecoveryKey {
    /// A fresh recovery key.
    pub fn generate() -> Result<Self> {
        let key = crypto::random_bytes::<20>()?;
        Ok(Self::from_raw(&key))
    }

    fn from_raw(key: &[u8; 20]) -> Self {
        let check = crypto::sha256(key);
        let mut bytes = Zeroizing::new(Vec::with_capacity(25));
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&check[..5]);
        let mut out = String::with_capacity(47);
        let mut acc: u64 = 0;
        let mut bits = 0;
        let mut n = 0;
        for &b in bytes.iter() {
            acc = (acc << 8) | u64::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                if n > 0 && n % 5 == 0 {
                    out.push('-');
                }
                out.push(CROCKFORD[((acc >> bits) & 31) as usize] as char);
                n += 1;
            }
        }
        Self(Zeroizing::new(out))
    }

    /// Parses user input (case-insensitive, dashes and spaces ignored,
    /// O/I/L read as 0/1/1). Rejects typos via the checksum.
    pub fn parse(input: &str) -> Result<Self> {
        let mut acc: u64 = 0;
        let mut bits = 0;
        let mut bytes = Zeroizing::new(Vec::with_capacity(25));
        for c in input.chars().filter(|c| !matches!(c, '-' | ' ')) {
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            let v = CROCKFORD
                .iter()
                .position(|&x| x as char == c)
                .ok_or(Error::WrongCredential)? as u64;
            acc = (acc << 5) | v;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                bytes.push(((acc >> bits) & 0xff) as u8);
            }
        }
        if bytes.len() != 25 {
            return Err(Error::WrongCredential);
        }
        let key: [u8; 20] = bytes[..20].try_into().expect("20 bytes");
        if !crypto::ct_eq(&crypto::sha256(&key)[..5], &bytes[20..]) {
            return Err(Error::WrongCredential);
        }
        Ok(Self::from_raw(&key))
    }

    /// The display form. Show it once; never log it.
    pub fn reveal(&self) -> &str {
        &self.0
    }

    fn kek(&self) -> Result<SecretKey> {
        // High-entropy input: HKDF is enough (no password stretching).
        let parsed = Self::parse(&self.0)?;
        let digest = Zeroizing::new(crypto::sha256(parsed.0.as_bytes()));
        Ok(SecretKey::from_bytes(digest.as_ref())?.derive(b"cua-keyvault/v1/recovery-kek"))
    }
}

/// The recovery-key protector.
pub struct RecoveryProtector(pub RecoveryKey);

impl Protector for RecoveryProtector {
    fn kind(&self) -> ProtectorKind {
        ProtectorKind::Recovery
    }

    fn enroll(&self, vault_id: &str, vmk: &SecretKey) -> Result<ProtectorRecord> {
        wrap_for(
            ProtectorKind::Recovery,
            "Recovery key",
            vault_id,
            &self.0.kek()?,
            vmk,
        )
    }

    fn unwrap(&self, vault_id: &str, record: &ProtectorRecord) -> Result<SecretKey> {
        unwrap_for(&self.0.kek()?, vault_id, record)
    }
}

/// Refuse to create the OS key protector from code that is not team-signed
/// (red-team F2). The keychain's default ACL trusts the *creating binary* by
/// its designated requirement; for a properly team-signed daemon that is a
/// code-requirement ACL, but for the unsigned standalone CLI it degrades to a
/// path-based trust, so a same-user attacker who can place a binary at that
/// path (before the signed app is installed, or by replacing it) becomes a
/// trusted reader of the KEK with no prompt. Until a signed identity is
/// present, the vault falls back to a passphrase, and the OS protector is
/// re-enrolled once the signed app is installed.
pub fn require_signed_os_protector(signing: &crate::caller::Signing) -> Result<()> {
    match signing {
        crate::caller::Signing::Signed { .. } => Ok(()),
        _ => Err(Error::Unsupported(
            "the OS key protector can only be created by the signed Cua daemon; create the vault \
             with a passphrase (`cua keyvault init --passphrase`) and re-enrol the OS protector \
             once the signed Cua app is installed"
                .into(),
        )),
    }
}

/// The keychain item service the macOS and Windows protectors use.
pub const OS_SECRET_SERVICE: &str = "Cua Keyvault";

/// The OS protector for this platform, or a typed [`Error::Unsupported`].
/// `keychain_path` selects an explicit keychain file on macOS (tests pass a
/// throwaway keychain; production passes `None`, the default keychain).
pub fn os_protector(keychain_path: Option<std::path::PathBuf>) -> Result<Box<dyn Protector>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::macos::KeychainProtector::new(
            keychain_path,
        )))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = keychain_path;
        Ok(Box::new(windows::CredentialProtector))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = keychain_path;
        Err(Error::Unsupported(
            "no OS-bound key protector on this platform yet (Secret Service and TPM2 are planned); \
             create the vault with a passphrase: `cua keyvault init --passphrase`"
                .into(),
        ))
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    /// Windows Credential Manager (DPAPI, current user) holds the KEK.
    pub struct CredentialProtector;

    impl Protector for CredentialProtector {
        fn kind(&self) -> ProtectorKind {
            ProtectorKind::WindowsCredential
        }

        fn enroll(&self, vault_id: &str, vmk: &SecretKey) -> Result<ProtectorRecord> {
            let kek = SecretKey::generate()?;
            let entry = keyring::Entry::new(OS_SECRET_SERVICE, vault_id)
                .map_err(|e| Error::Os(e.to_string()))?;
            entry
                .set_secret(kek.expose())
                .map_err(|e| Error::Os(e.to_string()))?;
            let mut rec = wrap_for(
                ProtectorKind::WindowsCredential,
                "Windows Credential Manager",
                vault_id,
                &kek,
                vmk,
            )?;
            rec.service = Some(OS_SECRET_SERVICE.into());
            rec.account = Some(vault_id.into());
            Ok(rec)
        }

        fn unwrap(&self, vault_id: &str, record: &ProtectorRecord) -> Result<SecretKey> {
            let entry = keyring::Entry::new(OS_SECRET_SERVICE, vault_id)
                .map_err(|e| Error::Os(e.to_string()))?;
            let raw = Zeroizing::new(entry.get_secret().map_err(|e| Error::Os(e.to_string()))?);
            unwrap_for(&SecretKey::from_bytes(&raw)?, vault_id, record)
        }

        fn remove(&self, record: &ProtectorRecord) -> Result<()> {
            if let Some(e) = record
                .account
                .as_ref()
                .and_then(|acct| keyring::Entry::new(OS_SECRET_SERVICE, acct).ok())
            {
                let _ = e.delete_credential();
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_key_round_trips_and_rejects_typos() {
        let k = RecoveryKey::generate().unwrap();
        let s = k.reveal().to_string();
        assert_eq!(s.len(), 47, "{s}");
        assert_eq!(s.matches('-').count(), 7);
        let back = RecoveryKey::parse(&s.to_lowercase().replace('-', " ")).unwrap();
        assert_eq!(back.reveal(), s);
        let mut typo: Vec<char> = s.chars().collect();
        typo[3] = if typo[3] == 'A' { 'B' } else { 'A' };
        assert!(RecoveryKey::parse(&typo.into_iter().collect::<String>()).is_err());
        assert!(RecoveryKey::parse("short").is_err());
        assert_eq!(format!("{k:?}"), "RecoveryKey(<redacted>)");
    }

    #[test]
    fn protectors_wrap_and_unwrap_and_bind_the_vault() {
        let vmk = SecretKey::generate().unwrap();
        let pp =
            PassphraseProtector::with_params("hunter2hunter2", KdfParams::for_tests().unwrap());
        let rec = pp.enroll("vault-a", &vmk).unwrap();
        assert_eq!(pp.unwrap("vault-a", &rec).unwrap().expose(), vmk.expose());
        assert!(matches!(
            pp.unwrap("vault-b", &rec),
            Err(Error::WrongCredential)
        ));
        let wrong = PassphraseProtector::new("not the passphrase");
        assert!(matches!(
            wrong.unwrap("vault-a", &rec),
            Err(Error::WrongCredential)
        ));
        assert!(PassphraseProtector::new("short").enroll("v", &vmk).is_err());

        let rk = RecoveryProtector(RecoveryKey::generate().unwrap());
        let rec = rk.enroll("vault-a", &vmk).unwrap();
        assert_eq!(rk.unwrap("vault-a", &rec).unwrap().expose(), vmk.expose());
        let other = RecoveryProtector(RecoveryKey::generate().unwrap());
        assert!(other.unwrap("vault-a", &rec).is_err());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_has_a_typed_unsupported_os_protector() {
        assert!(matches!(os_protector(None), Err(Error::Unsupported(_))));
    }

    #[test]
    fn os_protector_refuses_unsigned_enrolment() {
        use crate::caller::Signing;
        // Red-team F2: only a team-signed identity may create the KEK protector.
        assert!(
            require_signed_os_protector(&Signing::Signed {
                team_id: "YCK386LBJ7".into(),
                identifier: "com.trycua.cua".into(),
                cdhash: "00".into(),
            })
            .is_ok()
        );
        for s in [
            Signing::Unsigned,
            Signing::Unknown,
            Signing::AdHoc {
                identifier: "x".into(),
                cdhash: "y".into(),
            },
        ] {
            assert!(
                matches!(require_signed_os_protector(&s), Err(Error::Unsupported(_))),
                "{s:?} must be refused"
            );
        }
    }
}
