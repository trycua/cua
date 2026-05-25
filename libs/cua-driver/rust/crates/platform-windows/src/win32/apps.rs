//! Enumerate running processes on Windows.
//!
//! Uses CreateToolhelp32Snapshot / Process32FirstW / Process32NextW (simpler
//! and more portable than NtQuerySystemInformation or EnumProcesses+psapi).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

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
    let mut result = vec![root_pid];
    let mut frontier = vec![root_pid];
    while let Some(parent) = frontier.pop() {
        for p in &all {
            if p.parent_pid == parent && !result.contains(&p.pid) {
                result.push(p.pid);
                frontier.push(p.pid);
            }
        }
    }
    result
}

/// Like `list_descendants` but ALSO returns processes whose executable name
/// matches a prefix derived from `exe_basename`. This catches the LibreOffice
/// pattern (swriter.exe spawns soffice.bin via a parent-pid relationship we
/// might miss if the spawn happened before our pre-launch snapshot, OR via
/// CreateProcess flags that detach the child from the launcher's tree) as
/// well as the GIMP pattern (gimp-3.exe spawns gimp-3.2.exe whose name starts
/// with the same prefix).
///
/// Heuristic: strip extension and trailing version digits/dots/dashes from
/// the basename to derive a stable prefix. E.g.:
///   `gimp-3.exe`     → prefix `gimp`
///   `gimp-3.2.exe`   → prefix `gimp`
///   `swriter.exe`    → prefix `swriter` (won't match `soffice.bin` — that's
///                      LibreOffice's parent-pid path; usually still reachable
///                      via `list_descendants`)
///   `notepad++.exe`  → prefix `notepad++` (no version stripping needed)
///
/// Returns deduplicated pids; ordering favors descendants over name-matches.
pub fn related_processes(root_pid: u32, exe_basename: &str) -> Vec<u32> {
    let mut out = list_descendants(root_pid);
    let prefix = strip_version_suffix(exe_basename);
    if !prefix.is_empty() {
        let all = list_processes();
        for p in &all {
            let p_prefix = strip_version_suffix(&p.name);
            if p_prefix.eq_ignore_ascii_case(&prefix) && !out.contains(&p.pid) {
                out.push(p.pid);
            }
        }
    }
    out
}

/// Strip `.exe` (case-insensitive) and any trailing `-<digits>.<digits>...`
/// or `<digits>.<digits>...` version suffix. Used by `related_processes` to
/// match `gimp-3.exe` and `gimp-3.2.exe` under the common prefix `gimp`.
fn strip_version_suffix(basename: &str) -> String {
    let mut s = basename.to_ascii_lowercase();
    if let Some(stripped) = s.strip_suffix(".exe") {
        s = stripped.to_owned();
    }
    // Strip trailing version-like tail: `-3`, `-3.2`, `3`, `3.2`, etc.
    let bytes = s.as_bytes();
    let mut cut = bytes.len();
    while cut > 0 {
        let c = bytes[cut - 1] as char;
        if c.is_ascii_digit() || c == '.' || c == '-' { cut -= 1; } else { break; }
    }
    // Avoid stripping an entire name (e.g. "7z" → "" would lose information).
    // If everything past cut is purely digits/dots/dashes AND cut > 0, accept.
    if cut == 0 { return s; }
    s[..cut].to_string()
}
