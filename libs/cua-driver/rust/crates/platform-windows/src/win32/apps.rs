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
