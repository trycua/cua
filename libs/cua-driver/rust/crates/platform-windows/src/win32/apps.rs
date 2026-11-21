// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Enumerate running processes on Windows.
//!
//! Uses CreateToolhelp32Snapshot / Process32FirstW / Process32NextW (simpler
//! and more portable than NtQuerySystemInformation or EnumProcesses+psapi).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

pub use crate::process_selection::ProcessInfo;
use crate::process_selection::{descendants_from_processes, related_processes_from_processes};

/// Return all running processes (pid, parent_pid, executable name).
pub fn list_processes() -> Vec<ProcessInfo> {
    let mut result = Vec::new();
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return result,
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = decode_wstr(&entry.szExeFile);
                result.push(ProcessInfo {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    name,
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    result
}

/// Package identity Windows attests for a running process. Both fields are
/// `None` for an unpackaged (classic Win32) process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageIdentity {
    /// Package family name, e.g. `Microsoft.WindowsNotepad_8wekyb3d8bbwe`.
    pub family_name: Option<String>,
    /// Application User Model ID, `{PackageFamilyName}!{AppId}`. `None` for a
    /// packaged process that was not activated as an application.
    pub app_user_model_id: Option<String>,
}

/// Read the package identity of `pid` from its process token. Unlike the
/// executable name, this cannot be claimed by an unpackaged process.
pub fn process_package_identity(pid: u32) -> PackageIdentity {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::Storage::Packaging::Appx::{
        GetApplicationUserModelId, GetPackageFamilyName,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return PackageIdentity::default();
        };
        // PACKAGE_FAMILY_NAME_MAX_LENGTH is 64 and
        // APPLICATION_USER_MODEL_ID_MAX_LENGTH is 130 (plus the terminator).
        let mut family = [0_u16; 128];
        let mut family_len = family.len() as u32;
        let family_name =
            (GetPackageFamilyName(process, &mut family_len, Some(PWSTR(family.as_mut_ptr())))
                == ERROR_SUCCESS)
                .then(|| decode_wstr(&family))
                .filter(|value| !value.is_empty());
        let mut aumid = [0_u16; 256];
        let mut aumid_len = aumid.len() as u32;
        let app_user_model_id =
            (GetApplicationUserModelId(process, &mut aumid_len, Some(PWSTR(aumid.as_mut_ptr())))
                == ERROR_SUCCESS)
                .then(|| decode_wstr(&aumid))
                .filter(|value| !value.is_empty());
        let _ = CloseHandle(process);
        PackageIdentity {
            family_name,
            app_user_model_id,
        }
    }
}

fn decode_wstr(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Return all transitive descendants of `root_pid` (BFS through the process
/// tree). Includes processes that may have been spawned *after* `root_pid`
/// itself exited — useful for tracking launcher-stub chains where the
/// originally-launched binary re-execs into another process and exits (GIMP's
/// `gimp-3.exe` → `gimp-3.2.exe`; LibreOffice's `swriter.exe` → `soffice.bin`).
///
/// The result is in arrival order, which on Windows tends to correlate with
/// process-creation order — useful when picking the "main" descendant to
/// query for windows. Always includes `root_pid` itself first (even if it's
/// no longer alive, callers handle the empty-windows case the same way).
pub fn list_descendants(root_pid: u32) -> Vec<u32> {
    let all = list_processes();
    descendants_from_processes(root_pid, &all)
}

/// Return the launch receipt root and its transitive descendants, root first.
/// Executable-name similarity does not establish launch ancestry.
pub fn related_processes(root_pid: u32, _exe_basename: &str) -> Vec<u32> {
    related_processes_from_processes(root_pid, _exe_basename, &list_processes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpackaged_test_process_has_no_package_identity() {
        assert_eq!(
            process_package_identity(std::process::id()),
            PackageIdentity::default()
        );
    }
}
