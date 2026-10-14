// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS: the Keychain-wrapped key protector and code-signature
//! verification of socket peers.

use std::os::fd::RawFd;
use std::path::PathBuf;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::data::CFData;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::passwords::find_generic_password;
use zeroize::Zeroizing;

use crate::caller::{CallerIdentity, PeerError, Signing, TrustPolicy};
use crate::crypto::SecretKey;
use crate::protector::{OS_SECRET_SERVICE, Protector, ProtectorKind, ProtectorRecord};
use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Keychain protector
// ---------------------------------------------------------------------------

/// A random KEK in a Keychain generic-password item. Items added through
/// `SecKeychainAddGenericPassword` get an ACL that trusts only the creating
/// application, so another app reading it triggers a user prompt.
pub struct KeychainProtector {
    keychain_path: Option<PathBuf>,
}

impl KeychainProtector {
    /// `None` is the user's default keychain (production only: refused in
    /// tests); `Some(path)` an explicit keychain file.
    pub fn new(keychain_path: Option<PathBuf>) -> Self {
        Self { keychain_path }
    }

    fn open(&self, path: Option<&str>) -> Result<SecKeychain> {
        match path {
            Some(p) => SecKeychain::open(p).map_err(|e| Error::Os(format!("open keychain: {e}"))),
            None => {
                if crate::host_effects_forbidden() {
                    return Err(Error::HostEffectsRefused(
                        "the default (login) keychain is never used in tests; pass a throwaway keychain"
                            .into(),
                    ));
                }
                SecKeychain::default().map_err(|e| Error::Os(format!("default keychain: {e}")))
            }
        }
    }
}

impl Protector for KeychainProtector {
    fn kind(&self) -> ProtectorKind {
        ProtectorKind::MacosKeychain
    }

    fn enroll(&self, vault_id: &str, vmk: &SecretKey) -> Result<ProtectorRecord> {
        // Red-team F2: only the signed daemon may create the OS protector, so
        // an unsigned CLI cannot leave a path-trusted KEK behind.
        crate::protector::require_signed_os_protector(&self_signing())?;
        let path = self
            .keychain_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let kc = self.open(path.as_deref())?;
        let kek = SecretKey::generate()?;
        kc.set_generic_password(OS_SECRET_SERVICE, vault_id, kek.expose())
            .map_err(|e| Error::Os(format!("store the vault key in the keychain: {e}")))?;
        let mut rec = crate::protector::wrap_for(
            ProtectorKind::MacosKeychain,
            "macOS Keychain",
            vault_id,
            &kek,
            vmk,
        )?;
        rec.service = Some(OS_SECRET_SERVICE.into());
        rec.account = Some(vault_id.into());
        rec.keychain_path = path;
        Ok(rec)
    }

    fn unwrap(&self, vault_id: &str, record: &ProtectorRecord) -> Result<SecretKey> {
        let kc = self.open(record.keychain_path.as_deref())?;
        let service = record.service.as_deref().unwrap_or(OS_SECRET_SERVICE);
        let account = record.account.as_deref().unwrap_or(vault_id);
        let (pw, _item) = find_generic_password(Some(&[kc]), service, account)
            .map_err(|e| Error::Os(format!("read the vault key from the keychain: {e}")))?;
        let raw = Zeroizing::new(pw.as_ref().to_vec());
        crate::protector::unwrap_for(&SecretKey::from_bytes(&raw)?, vault_id, record)
    }

    fn remove(&self, record: &ProtectorRecord) -> Result<()> {
        let kc = self.open(record.keychain_path.as_deref())?;
        let service = record.service.as_deref().unwrap_or(OS_SECRET_SERVICE);
        if let Some(account) = record.account.as_deref()
            && let Ok((_, item)) = find_generic_password(Some(&[kc]), service, account)
        {
            item.delete();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Anti-rollback generation anchor (red-team F5)
// ---------------------------------------------------------------------------

/// The keychain item service holding the monotonic generation counter. It is a
/// separate item from the KEK, in the login keychain, so restoring the vault
/// directory cannot roll the counter back.
pub const GENERATION_SERVICE: &str = "Cua Keyvault Generation";

/// A monotonic per-vault generation counter in a login-keychain item.
pub struct KeychainGenerationAnchor {
    keychain_path: Option<PathBuf>,
}

impl KeychainGenerationAnchor {
    /// `None` is the default (login) keychain; `Some(path)` a throwaway one.
    pub fn new(keychain_path: Option<PathBuf>) -> Self {
        Self { keychain_path }
    }

    fn open(&self) -> Result<SecKeychain> {
        match self
            .keychain_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
        {
            Some(p) => SecKeychain::open(&p).map_err(|e| Error::Os(format!("open keychain: {e}"))),
            None => {
                if crate::host_effects_forbidden() {
                    return Err(Error::HostEffectsRefused(
                        "the default (login) keychain is never used in tests".into(),
                    ));
                }
                SecKeychain::default().map_err(|e| Error::Os(format!("default keychain: {e}")))
            }
        }
    }
}

impl crate::rollback::GenerationAnchor for KeychainGenerationAnchor {
    fn last(&self, vault_id: &str) -> Result<u64> {
        let kc = self.open()?;
        match find_generic_password(Some(&[kc]), GENERATION_SERVICE, vault_id) {
            Ok((pw, _)) => Ok(std::str::from_utf8(pw.as_ref())
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0)),
            // A missing item means no counter yet.
            Err(_) => Ok(0),
        }
    }

    fn record(&self, vault_id: &str, generation: u64) -> Result<()> {
        if generation <= self.last(vault_id)? {
            return Ok(());
        }
        let kc = self.open()?;
        kc.set_generic_password(
            GENERATION_SERVICE,
            vault_id,
            generation.to_string().as_bytes(),
        )
        .map_err(|e| Error::Os(format!("record the generation counter: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Peer identity
// ---------------------------------------------------------------------------

const SOL_LOCAL: libc::c_int = 0;
const LOCAL_PEERTOKEN: libc::c_int = 0x006;
/// `kSecCSSigningInformation`.
const K_SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;
/// `kSecCodeSignatureRuntime` in the signing flags.
const CS_RUNTIME: u32 = 0x0001_0000;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopySigningInformation(
        code: *const std::ffi::c_void,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
    fn SecCertificateCopySubjectSummary(cert: *const std::ffi::c_void) -> CFStringRef;
    static kSecCodeInfoIdentifier: CFStringRef;
    static kSecCodeInfoTeamIdentifier: CFStringRef;
    static kSecCodeInfoUnique: CFStringRef;
    static kSecCodeInfoFlags: CFStringRef;
    static kSecCodeInfoCertificates: CFStringRef;
}

/// The 32-byte `audit_token_t` of the peer.
fn peer_audit_token(fd: RawFd) -> std::result::Result<[u8; 32], PeerError> {
    let mut token = [0u8; 32];
    let mut len = token.len() as libc::socklen_t;
    // SAFETY: fd is a connected Unix socket owned by the caller; token and
    // len describe a writable 32-byte buffer.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            SOL_LOCAL,
            LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != token.len() {
        return Err(PeerError::Unavailable(format!(
            "LOCAL_PEERTOKEN: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(token)
}

fn audit_word(token: &[u8; 32], i: usize) -> u32 {
    u32::from_ne_bytes(token[i * 4..i * 4 + 4].try_into().expect("4 bytes"))
}

struct SigningInfo {
    identifier: Option<String>,
    team_id: Option<String>,
    cdhash: Option<String>,
    flags: u32,
    /// The leaf certificate's subject summary (the Developer ID / notarization
    /// common name Apple verified). The one identity string the signer cannot
    /// freely choose (red-team F4).
    verified_name: Option<String>,
}

/// The subject summary (common name) of the signing leaf certificate.
fn leaf_common_name(
    dict: &CFDictionary<CFString, CFType>,
    certs_key: CFStringRef,
) -> Option<String> {
    // SAFETY: certs_key is a valid CFString for the process lifetime.
    let key = unsafe { CFString::wrap_under_get_rule(certs_key) };
    let certs = dict.find(key)?.downcast::<CFArray>()?;
    if certs.is_empty() {
        return None;
    }
    // Leaf is index 0. The array holds SecCertificateRefs.
    let leaf = certs.get(0)?;
    let cn: CFStringRef =
        // SAFETY: leaf is a SecCertificateRef; the call returns a +1 CFString
        // (or null) that we wrap under the create rule.
        unsafe { SecCertificateCopySubjectSummary(*leaf) };
    if cn.is_null() {
        return None;
    }
    // SAFETY: create rule: we own the returned CFString.
    let cn = unsafe { CFString::wrap_under_create_rule(cn) };
    Some(cn.to_string())
}

fn signing_info(code: &SecCode) -> Option<SigningInfo> {
    let mut dict: CFDictionaryRef = std::ptr::null();
    // SAFETY: code is a valid SecCodeRef (a SecCode is accepted wherever a
    // SecStaticCode is); dict receives a +1 CFDictionary we wrap below.
    let rc = unsafe {
        SecCodeCopySigningInformation(
            code.as_concrete_TypeRef().cast(),
            K_SEC_CS_SIGNING_INFORMATION,
            &mut dict,
        )
    };
    if rc != 0 || dict.is_null() {
        return None;
    }
    // SAFETY: create rule: we own the returned dictionary.
    let dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(dict) };
    // SAFETY: the extern statics are valid CFStrings for the process lifetime.
    let key = |k: CFStringRef| unsafe { CFString::wrap_under_get_rule(k) };
    let string = |k: CFStringRef| {
        dict.find(key(k))
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
    };
    // SAFETY: as above.
    let (id_k, team_k, uniq_k, flags_k, certs_k) = unsafe {
        (
            kSecCodeInfoIdentifier,
            kSecCodeInfoTeamIdentifier,
            kSecCodeInfoUnique,
            kSecCodeInfoFlags,
            kSecCodeInfoCertificates,
        )
    };
    let cdhash = dict
        .find(key(uniq_k))
        .and_then(|v| v.downcast::<CFData>())
        .map(|d| hex::encode(d.bytes()));
    let flags = dict
        .find(key(flags_k))
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n| n.to_i64())
        .unwrap_or(0) as u32;
    let verified_name = leaf_common_name(&dict, certs_k);
    Some(SigningInfo {
        identifier: string(id_k),
        team_id: string(team_k),
        cdhash,
        flags,
        verified_name,
    })
}

fn code_for_audit_token(token: &[u8; 32]) -> Option<SecCode> {
    let data = CFData::from_buffer(token);
    let mut attrs = GuestAttributes::new();
    attrs.set_audit_token(data.as_concrete_TypeRef());
    SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE).ok()
}

fn code_path(code: &SecCode) -> Option<String> {
    code.path(Flags::NONE)
        .ok()
        .and_then(|u| u.to_path())
        .map(|p| p.to_string_lossy().into_owned())
}

/// The signing state of the *current* process. Used to refuse creating the OS
/// key protector from an unsigned binary (red-team F2): the default keychain
/// ACL trusts the creating binary, which for an unsigned CLI degrades to path
/// trust a same-user attacker can occupy.
pub fn self_signing() -> Signing {
    let mut attrs = GuestAttributes::new();
    // SAFETY: getpid has no preconditions.
    let pid = unsafe { libc::getpid() };
    attrs.set_pid(pid);
    let Some(code) = SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE).ok() else {
        return Signing::Unknown;
    };
    let valid = SecRequirement::from_str_checked("always")
        .and_then(|a| code.check_validity(Flags::NONE, &a).ok())
        .is_some();
    let info = if valid { signing_info(&code) } else { None };
    match &info {
        Some(SigningInfo {
            identifier: Some(id),
            team_id: Some(team),
            cdhash,
            ..
        }) => Signing::Signed {
            team_id: team.clone(),
            identifier: id.clone(),
            cdhash: cdhash.clone().unwrap_or_default(),
        },
        Some(SigningInfo {
            identifier: Some(id),
            cdhash: Some(cd),
            ..
        }) => Signing::AdHoc {
            identifier: id.clone(),
            cdhash: cd.clone(),
        },
        _ => Signing::Unsigned,
    }
}

/// Identifies the peer of a connected Unix socket from its audit token.
pub fn identify_peer(
    fd: RawFd,
    policy: &TrustPolicy,
) -> std::result::Result<CallerIdentity, PeerError> {
    let token = peer_audit_token(fd)?;
    identify_audit_token(&token, policy)
}

/// Identifies the process behind an audit token.
pub fn identify_audit_token(
    token: &[u8; 32],
    policy: &TrustPolicy,
) -> std::result::Result<CallerIdentity, PeerError> {
    // audit_token_t: [auid, euid, egid, ruid, rgid, pid, asid, pidversion].
    let euid = audit_word(token, 1);
    let pid = audit_word(token, 5) as i32;
    // SAFETY: geteuid has no preconditions.
    let ours = unsafe { libc::geteuid() };
    if euid != ours {
        return Err(PeerError::WrongUser { peer: euid, ours });
    }
    let Some(code) = code_for_audit_token(token) else {
        // The process is gone (or exec'd since connecting): its pid version
        // no longer matches. Never fall back to the bare pid.
        return Err(PeerError::Unavailable(
            "the peer process no longer matches its connect-time audit token".into(),
        ));
    };
    let path = code_path(&code);
    // Dynamic validity with no requirement: is the running code validly
    // signed at all (and not modified in memory)?
    let valid = SecRequirement::from_str_checked("always")
        .and_then(|always| code.check_validity(Flags::NONE, &always).ok())
        .is_some();
    let info = if valid { signing_info(&code) } else { None };
    let signing = match &info {
        Some(SigningInfo {
            identifier: Some(identifier),
            team_id: Some(team_id),
            cdhash,
            ..
        }) => Signing::Signed {
            team_id: team_id.clone(),
            identifier: identifier.clone(),
            cdhash: cdhash.clone().unwrap_or_default(),
        },
        Some(SigningInfo {
            identifier: Some(identifier),
            cdhash: Some(cdhash),
            ..
        }) => Signing::AdHoc {
            identifier: identifier.clone(),
            cdhash: cdhash.clone(),
        },
        _ => Signing::Unsigned,
    };
    let hardened = info.as_ref().is_some_and(|i| i.flags & CS_RUNTIME != 0);
    // The verified leaf name is only meaningful for a validly-signed caller
    // (red-team F4). Never carry one for `Unsigned`.
    let verified_name = if matches!(signing, Signing::Signed { .. } | Signing::AdHoc { .. }) {
        info.as_ref().and_then(|i| i.verified_name.clone())
    } else {
        None
    };
    let meets_requirement = valid
        && SecRequirement::from_str_checked(&policy.macos_requirement)
            .is_some_and(|req| code.check_validity(Flags::NONE, &req).is_ok());
    let first_party = meets_requirement && (hardened || !policy.require_hardened_runtime);
    let launched_by = if first_party {
        parent_display(pid, policy)
    } else {
        None
    };
    Ok(CallerIdentity {
        pid,
        uid: euid,
        path,
        signing,
        first_party,
        os_verified: true,
        launched_by,
        verified_name,
    })
}

trait RequirementExt {
    fn from_str_checked(s: &str) -> Option<SecRequirement>;
}

impl RequirementExt for SecRequirement {
    fn from_str_checked(s: &str) -> Option<SecRequirement> {
        s.parse::<SecRequirement>().ok()
    }
}

/// The display identity of `pid`'s parent (display only: pid based, so a
/// race can mislabel it, but it never grants anything).
fn parent_display(pid: i32, _policy: &TrustPolicy) -> Option<String> {
    let ppid = parent_pid(pid)?;
    if ppid <= 1 {
        return None;
    }
    let mut attrs = GuestAttributes::new();
    attrs.set_pid(ppid);
    let code = SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE).ok()?;
    let info = signing_info(&code);
    let label = match info {
        Some(SigningInfo {
            identifier: Some(id),
            team_id: Some(team),
            ..
        }) => format!("{id} (team {team})"),
        Some(SigningInfo {
            identifier: Some(id),
            ..
        }) => format!("{id} (not team-signed)"),
        _ => code_path(&code).unwrap_or_else(|| format!("pid {ppid}")),
    };
    Some(label)
}

fn parent_pid(pid: i32) -> Option<i32> {
    // SAFETY: proc_pidinfo with PROC_PIDTBSDINFO writes a proc_bsdinfo into
    // the provided buffer of the stated size.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        );
        (n == size).then_some(info.pbi_ppid as i32)
    }
}
