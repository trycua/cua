// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows DPAPI for Chromium's `Local State` key, behind a trait so every
//! caller is testable on any platform.
//!
//! Chromium (components/os_crypt, `DecryptStringWithDPAPI` /
//! `EncryptStringWithDPAPI`) wraps its random 32-byte AES-256-GCM key with
//! `CryptProtectData` in the user's scope and stores it in `Local State`
//! (`os_crypt.encrypted_key`, base64 of `"DPAPI"` + the blob). [`SystemDpapi`]
//! is the real thing on Windows (`cfg(windows)`); everywhere else it refuses.
//! [`Local State`](local_state_key) reading and the key bookkeeping around it
//! are plain code.

use std::path::Path;

use cua_teleport_bundle::chromium_crypto::gcm;
use zeroize::Zeroizing;

/// DPAPI, user scope, no entropy.
pub trait Dpapi: Send + Sync {
    /// `CryptUnprotectData`.
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String>;
    /// `CryptProtectData`.
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String>;
}

/// The operating system's DPAPI (Windows only).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDpapi;

#[cfg(windows)]
impl Dpapi for SystemDpapi {
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};
        let input = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `input` borrows `blob` for the call; `out` is written by the
        // API and freed with `LocalFree` below.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("CryptUnprotectData failed (a different Windows user or machine?)".into());
        }
        // SAFETY: on success `pbData` points at `cbData` bytes we now own.
        let data = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        Ok(Zeroizing::new(data))
    }

    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};
        let input = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: as in `unprotect`.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("CryptProtectData failed".into());
        }
        let data = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        Ok(data)
    }
}

#[cfg(not(windows))]
impl Dpapi for SystemDpapi {
    fn unprotect(&self, _blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        Err("DPAPI is only available on Windows".into())
    }
    fn protect(&self, _plain: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI is only available on Windows".into())
    }
}

/// The AES-256-GCM key a `Local State` file holds, unwrapped with `dpapi`.
pub fn local_state_key(
    local_state: &Path,
    dpapi: &dyn Dpapi,
) -> Result<Zeroizing<[u8; gcm::KEY_LEN]>, String> {
    let json = std::fs::read(local_state)
        .map_err(|e| format!("reading {}: {e}", local_state.display()))?;
    let enc = gcm::encrypted_key_of(&json)
        .ok_or_else(|| "Local State has no os_crypt.encrypted_key".to_string())?;
    let blob = gcm::dpapi_blob(&enc).map_err(|e| e.to_string())?;
    let key = dpapi.unprotect(&blob)?;
    let key: [u8; gcm::KEY_LEN] = key
        .as_slice()
        .try_into()
        .map_err(|_| "the Local State key is not 32 bytes".to_string())?;
    Ok(Zeroizing::new(key))
}

/// The destination's key: the one its `Local State` already holds, or a fresh
/// random one wrapped with `dpapi` and written into `Local State` (creating
/// the file, keeping every other field of an existing one). Returns the key and
/// whether it was created.
pub fn ensure_local_state_key(
    local_state: &Path,
    dpapi: &dyn Dpapi,
    random: [u8; gcm::KEY_LEN],
) -> Result<(Zeroizing<[u8; gcm::KEY_LEN]>, bool), String> {
    if local_state.is_file() {
        let json = std::fs::read(local_state).map_err(|e| e.to_string())?;
        if gcm::encrypted_key_of(&json).is_some() {
            return Ok((local_state_key(local_state, dpapi)?, false));
        }
    }
    let blob = dpapi.protect(&random)?;
    let mut doc: serde_json::Value = std::fs::read(local_state)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    doc["os_crypt"]["encrypted_key"] = serde_json::Value::String(gcm::encrypted_key_value(&blob));
    if let Some(dir) = local_state.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        local_state,
        serde_json::to_vec(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok((Zeroizing::new(random), true))
}

/// A DPAPI stand-in for tests on every platform: "protection" is a reversible
/// XOR with a fixed pad, enough to prove the callers wrap and unwrap.
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeDpapi;

impl Dpapi for FakeDpapi {
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        let body = blob.strip_prefix(b"FAKE").ok_or("not a fake DPAPI blob")?;
        Ok(Zeroizing::new(body.iter().map(|b| b ^ 0x5a).collect()))
    }
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = b"FAKE".to_vec();
        out.extend(plain.iter().map(|b| b ^ 0x5a));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_state_key_unwraps_and_a_missing_one_is_created_keeping_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let ls = dir.path().join("Local State");
        std::fs::write(&ls, br#"{"profile":{"last_used":"Default"}}"#).unwrap();
        assert!(local_state_key(&ls, &FakeDpapi).is_err());
        let (key, created) = ensure_local_state_key(&ls, &FakeDpapi, [9u8; 32]).unwrap();
        assert!(created && *key == [9u8; 32]);
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&ls).unwrap()).unwrap();
        assert_eq!(doc["profile"]["last_used"], "Default", "the rest is kept");
        assert_eq!(*local_state_key(&ls, &FakeDpapi).unwrap(), [9u8; 32]);
        // An existing key is reused, never replaced.
        let (again, created) = ensure_local_state_key(&ls, &FakeDpapi, [1u8; 32]).unwrap();
        assert!(!created && *again == [9u8; 32]);
        // No Local State at all: created from scratch.
        let fresh = dir.path().join("new/Local State");
        assert!(
            ensure_local_state_key(&fresh, &FakeDpapi, [4u8; 32])
                .unwrap()
                .1
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_system_dpapi_refuses_off_windows() {
        assert!(SystemDpapi.unprotect(b"x").is_err());
    }

    /// Windows CI: the real DPAPI round trip (user scope).
    #[cfg(windows)]
    #[test]
    fn the_system_dpapi_round_trips_on_windows() {
        let blob = SystemDpapi
            .protect(b"0123456789abcdef0123456789abcdef")
            .unwrap();
        assert_ne!(&blob[..], b"0123456789abcdef0123456789abcdef");
        let plain = SystemDpapi.unprotect(&blob).unwrap();
        assert_eq!(&**plain, b"0123456789abcdef0123456789abcdef");
        assert!(SystemDpapi.unprotect(b"not a blob").is_err());
    }
}
