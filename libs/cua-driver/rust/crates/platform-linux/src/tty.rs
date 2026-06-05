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

/// Inject `text` into the terminal whose slave is `/dev/pts/<target_ptn>` by
/// borrowing the master fd held by `emulator_pid`. Returns `Ok(true)` once the
/// bytes are written, or `Ok(false)` if the master could not be borrowed
/// (unsupported kernel, denied by ptrace policy, or no matching master found)
/// so the caller can fall back to another path.
pub fn inject_via_master(emulator_pid: u32, target_ptn: u32, text: &str) -> anyhow::Result<bool> {
    let Some(pidfd) = pidfd_open(emulator_pid) else {
        return Ok(false);
    };

    let entries = match std::fs::read_dir(format!("/proc/{emulator_pid}/fd")) {
        Ok(entries) => entries,
        Err(_) => return Ok(false),
    };

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
        if pts_number(local.as_raw_fd()) == Some(target_ptn) {
            // `local` is our own dup of the emulator's master; writing to it
            // feeds the slave's input queue. Handing it to File transfers
            // ownership so only our dup is closed on drop.
            let mut master = unsafe { std::fs::File::from_raw_fd(local.into_raw_fd()) };
            master.write_all(text.as_bytes())?;
            return Ok(true);
        }
    }
    Ok(false)
}
