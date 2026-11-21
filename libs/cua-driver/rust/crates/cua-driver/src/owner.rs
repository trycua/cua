// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `serve --owner-pid PID`: the daemon shuts down once the process that owns
//! it exits.
//!
//! The owner is opened once, at startup, as a handle the OS ties to that
//! process (a process handle on Windows, a kqueue on macOS, a pidfd on
//! Linux), so a later process that reuses the PID is never mistaken for it.
//! An owner that has exited but is not yet reaped counts as gone.

use std::sync::OnceLock;

static OWNER_EXITED: OnceLock<tokio::sync::watch::Receiver<bool>> = OnceLock::new();

/// Start watching `pid`; fails when no such process is running.
pub(crate) fn watch(pid: u32) -> anyhow::Result<()> {
    let owner = imp::Owner::open(pid)
        .map_err(|error| anyhow::anyhow!("cannot watch owner process {pid}: {error}"))?;
    let (exited, receiver) = tokio::sync::watch::channel(false);
    OWNER_EXITED
        .set(receiver)
        .map_err(|_| anyhow::anyhow!("an owner process is already watched"))?;
    std::thread::Builder::new()
        .name("cua-owner-pid".into())
        .spawn(move || {
            owner.wait();
            let _ = exited.send(true);
        })?;
    Ok(())
}

/// Resolves once the watched owner has exited; never, when none is watched.
pub(crate) async fn exited() {
    if let Some(receiver) = OWNER_EXITED.get() {
        if receiver.clone().wait_for(|exited| *exited).await.is_ok() {
            return;
        }
    }
    std::future::pending().await
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
    };

    pub(super) struct Owner(HANDLE);

    // SAFETY: the handle is owned by this value and only waited on and closed.
    unsafe impl Send for Owner {}

    impl Owner {
        pub(super) fn open(pid: u32) -> std::io::Result<Self> {
            // SAFETY: plain Win32 call; the returned handle is owned by `Owner`.
            let owner = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
                .map(Self)
                .map_err(std::io::Error::from)?;
            // A process object outlives its process while any handle to it is
            // open, so a signalled handle is an owner that already exited.
            // SAFETY: `owner.0` is a live process handle opened with SYNCHRONIZE.
            if unsafe { WaitForSingleObject(owner.0, 0) } == WAIT_OBJECT_0 {
                return Err(std::io::Error::other("the process has already exited"));
            }
            Ok(owner)
        }

        pub(super) fn wait(self) {
            // SAFETY: `self.0` is a live process handle opened with SYNCHRONIZE.
            unsafe {
                WaitForSingleObject(self.0, INFINITE);
            }
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: `self.0` is owned and closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

    pub(super) struct Owner(OwnedFd);

    impl Owner {
        pub(super) fn open(pid: u32) -> std::io::Result<Self> {
            // SAFETY: kqueue returns a new descriptor or -1.
            let queue = unsafe { libc::kqueue() };
            if queue < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `queue` is a fresh descriptor owned from here on.
            let queue = unsafe { OwnedFd::from_raw_fd(queue) };
            let change = libc::kevent {
                ident: pid as libc::uintptr_t,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: registers one event; a process that is gone fails with ESRCH.
            let registered = unsafe {
                libc::kevent(
                    queue.as_raw_fd(),
                    &change,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if registered < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let owner = Self(queue);
            let now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            if owner.next_event(&now) != 0 {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(owner)
        }

        pub(super) fn wait(self) {
            while self.next_event(std::ptr::null()) == 0 {}
        }

        /// The registered event's count (1), 0 when `timeout` passes first,
        /// or -1 on an error other than EINTR.
        fn next_event(&self, timeout: *const libc::timespec) -> libc::c_int {
            // SAFETY: an all-zero kevent is a valid out buffer.
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            loop {
                // SAFETY: waits for the one registered event into `event`;
                // `timeout` is null or points at a valid timespec.
                let received = unsafe {
                    libc::kevent(
                        self.0.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        timeout,
                    )
                };
                if !interrupted(received) {
                    return received;
                }
            }
        }
    }

    fn interrupted(result: libc::c_int) -> bool {
        result < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

    /// A pidfd, or the bare PID on kernels older than 5.3, which lack `pidfd_open`.
    pub(super) enum Owner {
        Pidfd(OwnedFd),
        Polled(libc::pid_t),
    }

    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

    impl Owner {
        pub(super) fn open(pid: u32) -> std::io::Result<Self> {
            let pid = libc::pid_t::try_from(pid)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            // SAFETY: pidfd_open returns a new descriptor or -1.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd >= 0 {
                // SAFETY: `fd` is a fresh descriptor owned from here on.
                let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
                // pidfd_open accepts an exited owner that is not yet reaped.
                if exited(&fd, 0) {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                return Ok(Self::Pidfd(fd));
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ENOSYS) {
                return Err(error);
            }
            if !alive(pid) {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(Self::Polled(pid))
        }

        pub(super) fn wait(self) {
            match self {
                Self::Pidfd(fd) => while !exited(&fd, -1) {},
                Self::Polled(pid) => {
                    while alive(pid) {
                        std::thread::sleep(POLL_INTERVAL);
                    }
                }
            }
        }
    }

    /// Whether the pidfd turned readable, which it does once its process
    /// exits, within `timeout_ms` (-1 waits as long as it takes).
    fn exited(fd: &OwnedFd, timeout_ms: libc::c_int) -> bool {
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            // SAFETY: `poll` points at one valid pollfd.
            let ready = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
            if ready >= 0 {
                return ready > 0;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return true;
            }
        }
    }

    /// EPERM still proves the PID exists; only ESRCH proves it is gone. A
    /// zombie still answers `kill`, so its `/proc` state is checked too.
    pub(super) fn alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 checks existence without delivering a signal.
        let signalled = unsafe { libc::kill(pid, 0) };
        if signalled != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return false;
        }
        !std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| zombie(&stat))
    }

    /// The state field follows the parenthesised command, which may itself
    /// contain spaces or parentheses.
    fn zombie(stat: &str) -> bool {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| matches!(state, "Z" | "X"))
    }
}

#[cfg(test)]
mod tests {
    fn spawn_lasting(seconds: u32) -> std::process::Child {
        let mut command = if cfg!(windows) {
            let mut ping = std::process::Command::new("ping");
            ping.args(["-n", &(seconds + 1).to_string(), "127.0.0.1"]);
            ping
        } else {
            let mut sleep = std::process::Command::new("sleep");
            sleep.arg(seconds.to_string());
            sleep
        };
        command
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn child")
    }

    #[test]
    fn the_wait_returns_once_the_owner_exits() {
        let mut child = spawn_lasting(1);
        let owner = super::imp::Owner::open(child.id()).expect("open a running child");
        let waiter = std::thread::spawn(move || owner.wait());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            !waiter.is_finished(),
            "the wait returned while the owner ran"
        );
        child.wait().expect("child exits");
        waiter.join().expect("the wait returns");
    }

    /// Blocks until `child` exits, leaving it unreaped.
    fn wait_without_reaping(child: &std::process::Child) {
        #[cfg(unix)]
        {
            // SAFETY: an all-zero siginfo_t is a valid out buffer.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: WNOWAIT leaves the child waitable for `Child::wait`.
            let waited = unsafe {
                libc::waitid(
                    libc::P_PID,
                    child.id() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            assert_eq!(waited, 0, "waitid: {}", std::io::Error::last_os_error());
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;
            use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
            use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
            // SAFETY: `child` owns this live process handle for the whole call.
            let waited = unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), INFINITE) };
            assert_eq!(waited, WAIT_OBJECT_0);
        }
    }

    #[test]
    fn an_owner_that_exited_but_is_not_reaped_is_refused() {
        let mut child = spawn_lasting(0);
        wait_without_reaping(&child);
        let opened = super::imp::Owner::open(child.id());
        child.wait().expect("reap the child");
        assert!(opened.is_err(), "an exited owner was accepted");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_polling_fallback_counts_an_unreaped_owner_as_gone() {
        let mut child = spawn_lasting(0);
        wait_without_reaping(&child);
        let alive = super::imp::alive(child.id() as libc::pid_t);
        child.wait().expect("reap the child");
        assert!(!alive, "a zombie owner counted as alive");
    }

    #[test]
    fn a_pid_whose_process_is_gone_is_refused() {
        let mut child = spawn_lasting(0);
        let pid = child.id();
        child.wait().expect("child exits");
        drop(child);
        assert!(super::imp::Owner::open(pid).is_err());
    }
}
