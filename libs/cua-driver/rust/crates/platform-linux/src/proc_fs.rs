//! Linux process enumeration via the /proc filesystem.
//!
//! Each /proc/<pid>/status file contains Name:, Pid:, PPid: etc.
//! /proc/<pid>/cmdline is the full command line (NUL-separated).

use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
}

/// Return all running processes by reading /proc/<pid>/status.
pub fn list_processes() -> Vec<ProcessInfo> {
    let mut result = Vec::new();
    let proc_dir = Path::new("/proc");
    let entries = match fs::read_dir(proc_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid_str = name.to_string_lossy();
        let pid: u32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue, // Skip non-numeric entries.
        };

        let status_path = proc_dir.join(&*pid_str).join("status");
        let status = match fs::read_to_string(&status_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let proc_name = status.lines()
            .find(|l| l.starts_with("Name:"))
            .map(|l| l[5..].trim().to_owned())
            .unwrap_or_default();

        let cmdline_path = proc_dir.join(&*pid_str).join("cmdline");
        let cmdline = fs::read(cmdline_path).ok()
            .map(|b| {
                // cmdline is NUL-separated; first entry is argv[0].
                let s = String::from_utf8_lossy(&b);
                s.split('\0').next().unwrap_or("").trim().to_owned()
            })
            .unwrap_or_default();

        result.push(ProcessInfo { pid, name: proc_name, cmdline });
    }

    result.sort_by_key(|p| p.pid);
    result
}
