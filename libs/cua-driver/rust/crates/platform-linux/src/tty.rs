// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Focus-free keyboard injection into terminals the driver launched, by
//! borrowing the terminal emulator's PTY *master* fd and writing bytes to it.
//! The kernel delivers them to the slave (the shell's stdin) exactly as if
//! typed — no X focus change, and immune to `dev.tty.legacy_tiocsti` (unlike
//! the legacy `TIOCSTI` ioctl this replaces).
//!
//! The master is obtained with `pidfd_getfd(2)`. That call requires
//! ptrace-mode access to the target, which under the default
//! `kernel.yama.ptrace_scope=1` is granted for the caller's own descendants —
//! i.e. terminals the driver itself launched — with **no root and no special
//! capability**. For a terminal the driver did not launch, the borrow is
//! denied and we return `Ok(false)` so the caller can fall back: injecting into
//! someone else's terminal unprivileged is exactly what the kernel is designed
//! to prevent.

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

/// `pidfd_open(2)` — a handle to `pid` usable with `pidfd_getfd`.
fn pidfd_open(pid: u32) -> Option<OwnedFd> {
    // SAFETY: thin syscall wrapper; returns a fresh fd or -1 on error.
    let ret = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    (ret >= 0).then(|| unsafe { OwnedFd::from_raw_fd(ret as i32) })
}

/// `pidfd_getfd(2)` — duplicate `remote_fd` out of the target into our process.
fn pidfd_getfd(pidfd: &OwnedFd, remote_fd: i32) -> Option<OwnedFd> {
    // SAFETY: thin syscall wrapper; returns a fresh fd or -1 on error.
    let ret = unsafe { libc::syscall(libc::SYS_pidfd_getfd, pidfd.as_raw_fd(), remote_fd, 0) };
    (ret >= 0).then(|| unsafe { OwnedFd::from_raw_fd(ret as i32) })
}

/// pts index of a pty *master* fd, or `None` if `fd` is not a master.
fn pts_number(fd: i32) -> Option<u32> {
    let mut n: libc::c_uint = 0;
    // SAFETY: TIOCGPTN writes a c_uint through the pointer; it fails with
    // ENOTTY on anything that isn't a pty master, which we treat as "no match".
    let ret = unsafe { libc::ioctl(fd, libc::TIOCGPTN, &mut n as *mut libc::c_uint) };
    (ret == 0).then_some(n as u32)
}

/// Inject `text` into the only pty whose master `emulator_pid` holds. Returns
/// `Ok(true)` once the bytes are written, or `Ok(false)` if the masters could
/// not be borrowed (unsupported kernel, denied by ptrace policy) or the
/// emulator holds no pty or several, so the caller can fall back to another
/// path.
pub fn inject_via_sole_master(emulator_pid: u32, text: &str) -> anyhow::Result<bool> {
    let Some(pidfd) = pidfd_open(emulator_pid) else {
        return Ok(false);
    };

    let entries = match std::fs::read_dir(format!("/proc/{emulator_pid}/fd")) {
        Ok(entries) => entries,
        Err(_) => return Ok(false),
    };

    let mut masters = std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(remote_fd) = name.parse::<i32>() else {
            continue;
        };
        let Some(local) = pidfd_getfd(&pidfd, remote_fd) else {
            continue;
        };
        if let Some(ptn) = pts_number(local.as_raw_fd()) {
            masters.entry(ptn).or_insert(local);
        }
    }
    let mut masters = masters.into_values();
    let (Some(local), None) = (masters.next(), masters.next()) else {
        return Ok(false);
    };
    // `local` is our own dup of the emulator's master; writing to it feeds the
    // slave's input queue. Handing it to File transfers ownership so only our
    // dup is closed on drop.
    let mut master = unsafe { std::fs::File::from_raw_fd(local.into_raw_fd()) };
    master.write_all(text.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    fn open_pty() -> (OwnedFd, std::fs::File) {
        // SAFETY: posix_openpt returns a fresh fd or -1.
        let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        assert!(fd >= 0, "posix_openpt failed");
        let master = unsafe { OwnedFd::from_raw_fd(fd) };
        assert_eq!(unsafe { libc::unlockpt(master.as_raw_fd()) }, 0);
        let ptn = pts_number(master.as_raw_fd()).expect("master pts number");
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(format!("/dev/pts/{ptn}"))
            .expect("open slave");
        (master, slave)
    }

    fn read_input(slave: &mut std::fs::File) -> String {
        let mut buf = [0u8; 64];
        for _ in 0..100 {
            if let Ok(n) = slave.read(&mut buf) {
                return String::from_utf8_lossy(&buf[..n]).into_owned();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        String::new()
    }

    #[test]
    fn injects_only_when_the_process_holds_a_single_pty() {
        let pid = std::process::id();
        let (_master, mut slave) = open_pty();
        assert!(inject_via_sole_master(pid, "ls\r").unwrap());
        assert_eq!(read_input(&mut slave), "ls\n");

        let (_other_master, mut other_slave) = open_pty();
        assert!(!inject_via_sole_master(pid, "ls\r").unwrap());
        assert_eq!(read_input(&mut slave), "");
        assert_eq!(read_input(&mut other_slave), "");
    }
}
