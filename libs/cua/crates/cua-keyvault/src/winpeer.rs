// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows: who is on the other end of a Keyvault named pipe.
//!
//! The pipe tells the server its client's process id and the client the
//! server's (`GetNamedPipeClientProcessId`, `GetNamedPipeServerProcessId`).
//! From that id this module reads three things, none of them from the peer:
//!
//! - the account the process runs as, which must be this one (the pipe's
//!   access list already keeps other accounts out; this is the second check);
//! - the executable the process runs (`QueryFullProcessImageNameW`);
//! - that file's Authenticode signature (`WinVerifyTrust`, no user interface,
//!   no network): a signature that chains to a trusted root, and the
//!   publisher it names (the signing certificate's subject).
//!
//! Cua is the code signed by the publisher that signed this process
//! ([`TrustPolicy::windows_publisher`] pins a name instead). Windows has no
//! hardened runtime to ask about, so unlike on macOS this vouches for the
//! file the process started from; a running image cannot be replaced, but a
//! process of the same account can still inject code into another.

use std::ffi::c_void;
use std::os::windows::io::RawHandle;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_SHA1_HASH_PROP_ID,
    CertGetCertificateContextProperty, CertGetNameStringW,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};

use crate::caller::{CallerIdentity, PeerError, TrustPolicy, WindowsSigner, windows_signing};

/// Which end of the pipe the peer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeSide {
    /// The peer is the client (this process serves the pipe).
    Client,
    /// The peer is the server (this process connected to it).
    Server,
}

/// A handle that closes itself.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle was opened by this module and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

pub(crate) fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// A process token's `TOKEN_USER`, in memory aligned for the struct.
pub(crate) struct TokenUserBuf(Vec<u64>);

impl TokenUserBuf {
    /// The account's security identifier; valid while `self` is.
    pub(crate) fn sid(&self) -> *mut c_void {
        // SAFETY: the buffer was filled by GetTokenInformation(TokenUser).
        unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
    }
}

/// The account `process` (a handle with query rights) runs as.
pub(crate) fn token_user(process: HANDLE) -> Result<TokenUserBuf, String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` is a process handle; `token` receives a handle we own.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(format!(
            "OpenProcessToken: {}",
            std::io::Error::last_os_error()
        ));
    }
    let token = Owned(token);
    let mut len = 0u32;
    // SAFETY: the first call only asks how much room the answer needs.
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(format!(
            "GetTokenInformation: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    if unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) }
        == 0
    {
        return Err(format!(
            "GetTokenInformation: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(TokenUserBuf(buf))
}

/// The executable `process` runs, as the system reports it.
fn image_path(process: HANDLE) -> Result<PathBuf, String> {
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` UTF-16 units; the call updates `len` to the
    // number written.
    if unsafe { QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) } == 0 {
        return Err(format!(
            "QueryFullProcessImageNameW: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &buf[..len as usize],
    )))
}

/// The publisher and certificate of `path`'s Authenticode signature, when it
/// is signed and the signature chains to a root the system trusts. Nothing is
/// fetched from the network.
pub fn authenticode_signer(path: &Path) -> Option<WindowsSigner> {
    let file = wide(path.as_os_str());
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: file.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    // SAFETY: an all-zero WINTRUST_DATA is the documented empty value.
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous.pFile = &mut info;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` and `info` live through both calls; INVALID_HANDLE_VALUE
    // (-1) as the window means no user interface.
    let trust = unsafe {
        WinVerifyTrust(
            -1isize as HANDLE,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    let signer = if trust == 0 {
        signer_of(data.hWVTStateData)
    } else {
        None
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: as above; closing releases the state the verify call kept.
    unsafe {
        WinVerifyTrust(
            -1isize as HANDLE,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    signer
}

/// The first signer's certificate in the verification state `state`.
fn signer_of(state: HANDLE) -> Option<WindowsSigner> {
    // SAFETY: `state` is the state data of a successful WinVerifyTrust; the
    // helpers return pointers into it (or null), valid until it is closed.
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return None;
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*signer).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        Some(WindowsSigner {
            subject: cert_subject(cert)?,
            thumbprint: cert_thumbprint(cert)?,
        })
    }
}

/// The certificate's subject as the system shows it (a UAC prompt's publisher).
///
/// # Safety
/// `cert` must point to a live certificate context.
unsafe fn cert_subject(cert: *const CERT_CONTEXT) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` holds `buf.len()` UTF-16 units; the call NUL-terminates.
    let n = unsafe {
        CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    };
    (n > 1).then(|| from_wide(&buf))
}

/// The certificate's SHA-1 thumbprint, hex.
///
/// # Safety
/// `cert` must point to a live certificate context.
unsafe fn cert_thumbprint(cert: *const CERT_CONTEXT) -> Option<String> {
    let mut buf = [0u8; 32];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` bytes; the call writes the property into it.
    let ok = unsafe {
        CertGetCertificateContextProperty(
            cert,
            CERT_SHA1_HASH_PROP_ID,
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (ok != 0).then(|| hex::encode(&buf[..len as usize]))
}

/// This process's own signer: what a peer must match to be Cua.
fn own_signer() -> Option<&'static WindowsSigner> {
    static OWN: OnceLock<Option<WindowsSigner>> = OnceLock::new();
    OWN.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| authenticode_signer(&exe))
    })
    .as_ref()
}

/// Identifies the process on the other end of the pipe `pipe`.
pub fn identify_pipe_peer(
    pipe: RawHandle,
    policy: &TrustPolicy,
    side: PipeSide,
) -> Result<CallerIdentity, PeerError> {
    let mut pid = 0u32;
    // SAFETY: `pipe` is a live named pipe handle of this process; `pid` receives the id.
    let ok = unsafe {
        match side {
            PipeSide::Client => GetNamedPipeClientProcessId(pipe as HANDLE, &mut pid),
            PipeSide::Server => GetNamedPipeServerProcessId(pipe as HANDLE, &mut pid),
        }
    };
    if ok == 0 || pid == 0 {
        return Err(PeerError::Unavailable(format!(
            "the pipe's peer id: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: plain call; the handle is closed by `Owned`.
    let process = Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if process.0.is_null() {
        return Err(PeerError::Inspect(format!(
            "open process {pid}: {}",
            std::io::Error::last_os_error()
        )));
    }
    let theirs = token_user(process.0).map_err(PeerError::Inspect)?;
    // SAFETY: the pseudo handle of this process needs no closing.
    let ours = token_user(unsafe { GetCurrentProcess() }).map_err(PeerError::Inspect)?;
    // SAFETY: both SIDs live in their buffers, which outlive the call.
    if unsafe { EqualSid(theirs.sid(), ours.sid()) } == 0 {
        return Err(PeerError::Inspect(format!(
            "process {pid} runs as another account"
        )));
    }
    let path = image_path(process.0).map_err(PeerError::Inspect)?;
    let signer = authenticode_signer(&path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (signing, first_party) = windows_signing(signer.as_ref(), own_signer(), &file_name, policy);
    Ok(CallerIdentity {
        pid: pid as i32,
        uid: 0,
        path: Some(path.to_string_lossy().into_owned()),
        signing,
        first_party,
        os_verified: signer.is_some(),
        launched_by: None,
        verified_name: signer.map(|s| s.subject),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_file_is_signed_by_microsoft_and_a_plain_file_is_not() {
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let signer =
            authenticode_signer(&system.join(r"System32\notepad.exe")).expect("notepad is signed");
        assert!(signer.subject.contains("Microsoft"), "{signer:?}");
        assert_eq!(signer.thumbprint.len(), 40);

        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.exe");
        std::fs::write(&plain, b"MZ not a program").unwrap();
        assert_eq!(authenticode_signer(&plain), None);
        assert_eq!(authenticode_signer(&dir.path().join("missing.exe")), None);
    }

    #[test]
    fn this_test_process_is_unsigned_so_it_recognises_no_peer_as_cua() {
        assert_eq!(own_signer(), None);
    }
}
