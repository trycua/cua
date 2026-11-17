//! Size caps for the logs other processes write on this host.
//!
//! The daemon writes its own log through a self-rotating file
//! (`cua_vmm::disk::logs::RotatingFile`). These logs are written by
//! something else (launchd/systemd service stdout, `lume serve`, QEMU), so
//! they are capped from outside: copied to `<log>.1` and truncated in place,
//! which is safe for writers that append. A stopped VM's console log is cut
//! to its tail.

use std::path::PathBuf;

use cua_vmm::disk::logs;

use crate::layout::Layout;

/// Cap of one log file.
pub const MAX_BYTES: u64 = logs::DEFAULT_MAX_BYTES;
/// Rotated copies kept.
pub const KEEP: usize = logs::DEFAULT_KEEP;
/// What a stopped VM's console log is cut to.
pub const CONSOLE_TAIL: u64 = 1 << 20;
/// A running VM's console log is cut only past this size.
pub const RUNNING_CONSOLE_MAX: u64 = 10 * MAX_BYTES;

/// Caps every host log; returns the files it rotated or trimmed.
pub fn rotate_all(layout: &Layout) -> Vec<PathBuf> {
    let lume: Vec<PathBuf> = cua_vmm::lume::LUME_DAEMON_LOGS
        .iter()
        .map(PathBuf::from)
        .collect();
    rotate_all_with(layout, &lume)
}

/// [`rotate_all`] with the Lume installer's log paths given (tests).
pub fn rotate_all_with(layout: &Layout, lume_daemon_logs: &[PathBuf]) -> Vec<PathBuf> {
    let mut done = Vec::new();
    // Appended to by other processes: copy and truncate.
    let mut appended = vec![
        layout.host().join("driver.log"),
        layout.lume().join("serve.log"),
    ];
    appended.extend(
        layout
            .spacesd_logs()
            .into_iter()
            .filter(|p| p.extension().is_some_and(|x| x == "log")),
    );
    // The Lume installer's LaunchAgent logs, when cua installed Lume.
    if layout.lume().join("installed-by-cua").exists() {
        appended.extend(lume_daemon_logs.iter().cloned());
    }
    for p in appended {
        if logs::copy_truncate(&p, MAX_BYTES, KEEP).unwrap_or(false) {
            done.push(p);
        }
    }
    // QEMU instance logs: qemu.log is appended (`>>`). serial.log is
    // truncated by every launch and written at QEMU's own offset, so a
    // running instance's console is only cut when it is far over the cap
    // (the file becomes sparse: little disk, a hole for readers); a stopped
    // instance's console is cut to its tail.
    if let Ok(rd) = std::fs::read_dir(layout.qemu()) {
        for e in rd.flatten() {
            let dir = e.path();
            let running = std::fs::read(dir.join("state.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<cua_vmm::qemu::QemuState>(&b).ok())
                .and_then(|s| s.pid)
                .is_some_and(cua_vmm::host::pid_alive);
            let q = dir.join("qemu.log");
            if logs::copy_truncate(&q, MAX_BYTES, 1).unwrap_or(false) {
                done.push(q);
            }
            let s = dir.join("serial.log");
            let trimmed = if running {
                logs::copy_truncate(&s, RUNNING_CONSOLE_MAX, 1)
            } else {
                logs::trim_tail(&s, MAX_BYTES, CONSOLE_TAIL)
            };
            if trimmed.unwrap_or(false) {
                done.push(s);
            }
        }
    }
    // Lume Linux VMs' run logs (lume writes them while the VM runs).
    if let Ok(rd) = std::fs::read_dir(layout.lume()) {
        for e in rd.flatten() {
            let p = e.path().join("run.log");
            if logs::copy_truncate(&p, MAX_BYTES, 1).unwrap_or(false) {
                done.push(p);
            }
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_service_and_vm_logs() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        std::fs::create_dir_all(l.host()).unwrap();
        std::fs::write(
            l.host().join("driver.log"),
            vec![b'x'; (MAX_BYTES + 10) as usize],
        )
        .unwrap();
        let vm = l.qemu().join("box");
        std::fs::create_dir_all(&vm).unwrap();
        std::fs::write(vm.join("serial.log"), vec![b's'; (MAX_BYTES + 10) as usize]).unwrap();
        std::fs::write(vm.join("qemu.log"), b"small").unwrap();
        let done = rotate_all(&l);
        assert_eq!(done.len(), 2, "{done:?}");
        assert_eq!(
            std::fs::metadata(l.host().join("driver.log"))
                .unwrap()
                .len(),
            0
        );
        assert!(l.host().join("driver.log.1").exists());
        assert_eq!(
            std::fs::metadata(vm.join("serial.log")).unwrap().len(),
            CONSOLE_TAIL
        );
        assert_eq!(std::fs::read(vm.join("qemu.log")).unwrap(), b"small");
    }

    #[test]
    fn lume_installer_logs_are_capped_only_when_cua_installed_lume() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("home"));
        let log = d.path().join("lume_daemon.log");
        std::fs::write(&log, vec![b'l'; (MAX_BYTES + 1) as usize]).unwrap();
        assert!(rotate_all_with(&l, std::slice::from_ref(&log)).is_empty());
        std::fs::create_dir_all(l.lume()).unwrap();
        std::fs::write(l.lume().join("installed-by-cua"), b"1").unwrap();
        assert_eq!(
            rotate_all_with(&l, std::slice::from_ref(&log)),
            vec![log.clone()]
        );
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
    }
}
