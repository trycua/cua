// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows: the Keyvault's named pipe, in place of `keyvault.sock`.
//!
//! - `\\.\pipe\cua-keyvault-<hash of the socket path>`: the socket path
//!   (`$CUA_HOME/keyvault.sock`) names the vault, so a test home or a second
//!   cua home gets a pipe of its own.
//! - Access: the current account only (a protected DACL with one entry), no
//!   remote clients, and the first instance may be created once, so a program
//!   cannot slip a second server under the daemon's name.
//! - Clients connect at the identification level of impersonation: a
//!   server that is not Cua cannot act as the user.
//! - Every peer is identified by its process before its first frame
//!   ([`crate::winpeer`]).

use std::ffi::c_void;
use std::path::Path;
use std::time::Duration;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions,
};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::winpeer::{token_user, wide};
use crate::{Error, Result};

/// `ERROR_PIPE_BUSY`: every instance is serving someone; try again shortly.
const ERROR_PIPE_BUSY: i32 = 231;

/// The pipe that stands for the socket at `path`.
pub fn pipe_name(path: &Path) -> String {
    let digest = crate::crypto::sha256(path.to_string_lossy().to_lowercase().as_bytes());
    format!(r"\\.\pipe\cua-keyvault-{}", &hex::encode(digest)[..16])
}

/// This account's security identifier as `S-1-5-21-...`.
fn current_user_sid() -> Result<String> {
    // SAFETY: the pseudo handle of this process needs no closing.
    let user = token_user(unsafe { GetCurrentProcess() }).map_err(Error::Os)?;
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: the SID is valid while `user` is; `text` receives a LocalAlloc'd string.
    if unsafe { ConvertSidToStringSidW(user.sid(), &mut text) } == 0 {
        return Err(Error::Os(format!(
            "ConvertSidToStringSid: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `text` is a NUL-terminated UTF-16 string we free right after.
    let sid = unsafe {
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        sid
    };
    Ok(sid)
}

/// The access list: generic-all for this account and no one else, not
/// inherited. (`D:P(A;;GA;;;<sid>)`)
fn sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// A new instance of the pipe `name`. The first is created with the
/// guarantee that no other server holds the name.
pub fn create_server(name: &str, first: bool) -> Result<NamedPipeServer> {
    let sddl = wide(std::ffi::OsStr::new(&sddl(&current_user_sid()?)));
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is a NUL-terminated UTF-16 string; `descriptor` receives a LocalAlloc'd one.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(Error::Os(format!(
            "the pipe's access list: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte);
    // SAFETY: `attributes` and the descriptor it points at live through the call.
    let made = unsafe {
        options.create_with_security_attributes_raw(
            name,
            (&mut attributes as *mut SECURITY_ATTRIBUTES).cast::<c_void>(),
        )
    };
    // SAFETY: the system copied the descriptor into the pipe; ours is freed once.
    unsafe { LocalFree(descriptor.cast()) };
    made.map_err(|e| Error::Os(format!("create the pipe {name}: {e}")))
}

/// Opens the pipe `name` as a client, waiting a little while every instance is busy.
pub async fn open_client(name: &str) -> std::io::Result<NamedPipeClient> {
    let mut tries = 0;
    loop {
        // SECURITY_IDENTIFICATION: the server may ask who this is, never act as them.
        match ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name)
        {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 40 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_path_has_a_pipe_of_its_own() {
        let a = pipe_name(Path::new(r"C:\Users\ada\.cua\keyvault.sock"));
        assert!(
            a.starts_with(r"\\.\pipe\cua-keyvault-")
                && a.len() == r"\\.\pipe\cua-keyvault-".len() + 16,
            "{a}"
        );
        // Case does not make another home on Windows.
        assert_eq!(a, pipe_name(Path::new(r"c:\users\ADA\.cua\keyvault.sock")));
        assert_ne!(a, pipe_name(Path::new(r"C:\Users\bob\.cua\keyvault.sock")));
    }

    #[test]
    fn the_access_list_is_this_account_only() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(sddl(&sid), format!("D:P(A;;GA;;;{sid})"));
    }

    #[tokio::test]
    async fn a_second_server_cannot_take_the_first_one_s_name() {
        let name = pipe_name(Path::new(&format!(
            r"C:\cua-test-{}\keyvault.sock",
            std::process::id()
        )));
        let _first = create_server(&name, true).unwrap();
        assert!(create_server(&name, true).is_err());
        // More instances of the same server are fine.
        let _more = create_server(&name, false).unwrap();
    }
}
