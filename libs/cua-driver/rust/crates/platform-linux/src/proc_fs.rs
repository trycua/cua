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

fn is_process_live_state(status: &str) -> bool {
    status
        .lines()
        .find_map(|line| {
            line.strip_prefix("State:")
                .and_then(|state| state.trim().chars().next())
        })
        .is_some_and(|state| !matches!(state, 'Z' | 'X'))
}

/// Return whether a PID still represents a live process.
///
/// A zombie remains visible under `/proc` until its parent reaps it, so an
/// existence check alone is not enough for callers that may query AT-SPI or
/// X11 state for the process. Treat zombies and dead processes as gone.
pub fn is_process_live(pid: u32) -> bool {
    let status = match fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => status,
        Err(_) => return false,
    };

    is_process_live_state(&status)
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

        if !is_process_live_state(&status) {
            continue;
        }

        let proc_name = status
            .lines()
            .find(|l| l.starts_with("Name:"))
            .map(|l| l[5..].trim().to_owned())
            .unwrap_or_default();

        let cmdline_path = proc_dir.join(&*pid_str).join("cmdline");
        let cmdline = fs::read(cmdline_path)
            .ok()
            .map(|b| {
                // cmdline is NUL-separated; first entry is argv[0].
                let s = String::from_utf8_lossy(&b);
                s.split('\0').next().unwrap_or("").trim().to_owned()
            })
            .unwrap_or_default();

        result.push(ProcessInfo {
            pid,
            name: proc_name,
            cmdline,
        });
    }

    result.sort_by_key(|p| p.pid);
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn process_states_fail_closed() {
        assert!(super::is_process_live_state(
            "Name:\ttest\nState:\tR (running)\n"
        ));
        assert!(super::is_process_live_state("State:\tS (sleeping)"));
        assert!(!super::is_process_live_state("State:\tZ (zombie)"));
        assert!(!super::is_process_live_state("State:\tX (dead)"));
        assert!(!super::is_process_live_state("Name:\ttest\n"));
        assert!(!super::is_process_live_state("State:\t"));
    }

    #[test]
    fn real_zombie_is_not_live_and_disappears_after_reaping() {
        let child = unsafe { libc::fork() };
        assert!(
            child >= 0,
            "fork failed: {}",
            std::io::Error::last_os_error()
        );
        if child == 0 {
            unsafe { libc::_exit(0) };
        }

        let pid = child as u32;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut observed_zombie = false;
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .is_some_and(|status| status.contains("State:\tZ"))
            {
                observed_zombie = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let live_while_zombie = super::is_process_live(pid);
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(child, &mut status, 0) };

        assert!(observed_zombie, "child never entered zombie state");
        assert!(!live_while_zombie, "zombie pid was reported live");
        assert_eq!(reaped, child, "failed to reap child");
        assert!(!super::is_process_live(pid), "reaped pid was reported live");
    }
}
