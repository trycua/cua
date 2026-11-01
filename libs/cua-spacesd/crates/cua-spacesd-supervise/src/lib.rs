// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! PID 1 duties for an OCI image that does not carry cua-spacesd.
//!
//! Cua injects a static `cua-spacesd` into such an image (container and
//! microVM Spaces) and makes it the entrypoint: `cua-spacesd supervise --
//! <the image's ENTRYPOINT and CMD>`. [`run`] is that process. It
//!
//! * starts the image's command as a child, with the image's user and
//!   working directory, and without the Space's token in its environment;
//! * starts `cua-spacesd serve` as a second child (as root, with the token)
//!   and starts it again, with a backoff, if it dies;
//! * reaps every child, orphaned grandchildren included (it is PID 1 in a
//!   container; elsewhere it becomes a subreaper);
//! * forwards `SIGTERM`, `SIGINT`, `SIGQUIT`, `SIGHUP`, `SIGUSR1`, `SIGUSR2`,
//!   `SIGWINCH`, `SIGALRM` and (Linux) `SIGPWR` and the real-time signals to
//!   the image's command, and stops cua-spacesd once the command has gone
//!   after a termination signal;
//! * ends with the command's exit status (`128 + n` when signalled), like
//!   the image would have, unless the policy is [`OnExit::Serve`]: then the
//!   Space keeps serving after a command that exits by itself (an image's
//!   default `CMD` such as `/bin/sh` or `node` exits at once without a
//!   terminal) and ends with `0` on a termination signal.
//!
//! One [`run`] per process: its signal handlers are process-wide.

#![cfg(unix)]

pub mod user;

use std::ffi::CString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// What happens when the image's command exits on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExit {
    /// The Space ends with the command's exit status.
    Exit,
    /// The Space keeps serving cua-spacesd.
    Serve,
}

/// The image's command.
#[derive(Clone, Debug, Default)]
pub struct Workload {
    /// `ENTRYPOINT` and `CMD` joined, or the Space's command.
    pub argv: Vec<String>,
    /// The image's `User`; empty or `None` runs it as the supervisor's user.
    pub user: Option<String>,
    /// The image's working directory.
    pub workdir: Option<PathBuf>,
    /// Variables the command must not see (the Space's token).
    pub env_remove: Vec<String>,
}

/// The cua-spacesd child.
#[derive(Clone, Debug)]
pub struct Server {
    /// The executable (this binary).
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Variables set only when the environment does not already have them.
    pub env_defaults: Vec<(String, String)>,
}

/// What [`run`] supervises.
#[derive(Clone, Debug)]
pub struct Config {
    /// The image's command; `None` serves cua-spacesd alone.
    pub workload: Option<Workload>,
    /// The cua-spacesd child; `None` supervises the command alone.
    pub server: Option<Server>,
    /// What an exit of the command by itself means.
    pub on_exit: OnExit,
    /// How long cua-spacesd has to stop after `SIGTERM` before `SIGKILL`.
    pub stop_grace: Duration,
}

static PIPE_W: AtomicI32 = AtomicI32::new(-1);

fn errno_slot() -> *mut libc::c_int {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        libc::__errno_location()
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    unsafe {
        libc::__error()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd"
    )))]
    unsafe {
        libc::__errno_location()
    }
}

/// Async-signal-safe: one byte per signal into the self-pipe.
extern "C" fn on_signal(sig: libc::c_int) {
    unsafe {
        let slot = errno_slot();
        let saved = *slot;
        let fd = PIPE_W.load(Ordering::Relaxed);
        if fd >= 0 {
            let b = sig as u8;
            let _ = libc::write(fd, (&b as *const u8).cast(), 1);
        }
        *slot = saved;
    }
}

/// The signals the supervisor catches: `SIGCHLD`, and every signal an
/// image's `STOPSIGNAL` or an operator might send, which go to the command
/// (the real-time ones too, which systemd-style images stop on).
fn handled_signals() -> Vec<libc::c_int> {
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "android")),
        allow(unused_mut)
    )]
    let mut v = vec![
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGHUP,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGWINCH,
        libc::SIGALRM,
        libc::SIGCHLD,
    ];
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        v.push(libc::SIGPWR);
        v.extend(libc::SIGRTMIN()..=libc::SIGRTMAX());
    }
    v
}

fn set_flags(fd: libc::c_int) -> io::Result<()> {
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl < 0 || libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Installs the handlers and returns the pipe's read end.
fn install_signals() -> io::Result<libc::c_int> {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    set_flags(fds[0])?;
    set_flags(fds[1])?;
    PIPE_W.store(fds[1], Ordering::Relaxed);
    for sig in handled_signals() {
        unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            act.sa_flags = libc::SA_RESTART | libc::SA_NOCLDSTOP;
            libc::sigfillset(&mut act.sa_mask);
            if libc::sigaction(sig, &act, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    // Orphans of a supervisor that is not PID 1 come here, not to init.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
    Ok(fds[0])
}

/// Waits up to `timeout` for the pipe and returns the signals in it.
fn wait_signals(fd: libc::c_int, timeout: Option<Duration>) -> Vec<libc::c_int> {
    let ms = match timeout {
        None => -1,
        Some(d) => d.as_millis().min(i32::MAX as u128) as libc::c_int + 1,
    };
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe {
        libc::poll(&mut p, 1, ms);
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        out.extend(buf[..n as usize].iter().map(|b| *b as libc::c_int));
    }
    out
}

fn say(msg: impl std::fmt::Display) {
    eprintln!("cua-spacesd supervise: {msg}");
}

/// The exit code a wait status stands for (`128 + n` for a signal).
pub fn decode_status(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

/// Drops to `id` between fork and exec: supplementary groups, gid, uid.
///
/// The standard streams are handed to the user first, as a container
/// runtime does for a command that runs as a user: images log to
/// `/dev/stdout` and `/dev/stderr` (nginx, for one), which the user could
/// not open while the pipes behind them belong to root.
fn drop_to(id: &user::Identity, name: Option<&CString>) -> io::Result<()> {
    unsafe {
        for fd in 0..3 {
            libc::fchown(fd, id.uid as _, id.gid as _);
        }
        let ok = match name {
            Some(n) => libc::initgroups(n.as_ptr(), id.gid as _) == 0,
            None => libc::setgroups(0, std::ptr::null()) == 0,
        };
        if !ok || libc::setgid(id.gid as _) != 0 || libc::setuid(id.uid as _) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn spawn_workload(w: &Workload) -> io::Result<u32> {
    let mut cmd = Command::new(&w.argv[0]);
    cmd.args(&w.argv[1..]);
    for k in &w.env_remove {
        cmd.env_remove(k);
    }
    if let Some(d) = &w.workdir {
        cmd.current_dir(d);
    }
    if let Some(spec) = w.user.as_deref().filter(|u| !u.trim().is_empty()) {
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
        let id = user::resolve(spec, &passwd, &group)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let (uid, gid) = unsafe { (libc::geteuid() as u32, libc::getegid() as u32) };
        if id.uid != uid || id.gid != gid {
            if let Some(h) = &id.home {
                cmd.env("HOME", h);
            }
            let name = id.name.as_ref().and_then(|n| CString::new(n.as_str()).ok());
            unsafe {
                cmd.pre_exec(move || drop_to(&id, name.as_ref()));
            }
        }
    }
    cmd.spawn().map(|c| c.id())
}

fn spawn_server(s: &Server) -> io::Result<u32> {
    let mut cmd = Command::new(&s.program);
    cmd.args(&s.args).stdin(Stdio::null());
    for (k, v) in &s.env_defaults {
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
    cmd.spawn().map(|c| c.id())
}

fn kill(pid: u32, sig: libc::c_int) {
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

struct Stopping {
    code: i32,
    kill_at: Instant,
    killed: bool,
}

/// Runs the Space's PID 1 until it should end; returns the exit status.
pub fn run(cfg: Config) -> i32 {
    let pipe = match install_signals() {
        Ok(fd) => fd,
        Err(e) => {
            say(format!("cannot install signal handlers: {e}"));
            return 1;
        }
    };
    let mut workload_pid: Option<u32> = None;
    let mut workload_code: Option<i32> = None;
    let mut server_pid: Option<u32> = None;
    let mut server_restart_at: Option<Instant> = Some(Instant::now());
    let mut server_started = Instant::now();
    let mut server_failures: u32 = 0;
    let mut stopping: Option<Stopping> = None;
    let mut terminating = false;

    if cfg.server.is_none() {
        server_restart_at = None;
    }
    if let Some(w) = cfg.workload.as_ref().filter(|w| !w.argv.is_empty()) {
        match spawn_workload(w) {
            Ok(pid) => workload_pid = Some(pid),
            Err(e) => {
                let code = if e.kind() == io::ErrorKind::PermissionDenied {
                    126
                } else {
                    127
                };
                say(format!("cannot start {:?}: {e}", w.argv[0]));
                workload_code = Some(code);
                if cfg.on_exit == OnExit::Exit {
                    stopping = Some(Stopping {
                        code,
                        kill_at: Instant::now() + cfg.stop_grace,
                        killed: false,
                    });
                }
            }
        }
    }

    loop {
        let now = Instant::now();
        // The server: (re)start it unless the Space is ending.
        if stopping.is_none() && server_pid.is_none() {
            if let (Some(at), Some(s)) = (server_restart_at, cfg.server.as_ref()) {
                if now >= at {
                    server_restart_at = None;
                    match spawn_server(s) {
                        Ok(pid) => {
                            server_pid = Some(pid);
                            server_started = now;
                        }
                        Err(e) => {
                            say(format!("cannot start cua-spacesd: {e}"));
                            server_failures += 1;
                            server_restart_at = Some(now + backoff(server_failures));
                        }
                    }
                }
            }
        }
        // A stopping Space ends once its server is gone; SIGKILL after the
        // grace.
        if let Some(st) = stopping.as_mut() {
            match server_pid {
                None => return st.code,
                Some(pid) if now >= st.kill_at && !st.killed => {
                    kill(pid, libc::SIGKILL);
                    st.killed = true;
                }
                _ => {}
            }
        }
        let timeout = [
            server_restart_at.filter(|_| stopping.is_none() && server_pid.is_none()),
            stopping
                .as_ref()
                .filter(|s| !s.killed && server_pid.is_some())
                .map(|s| s.kill_at),
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|t| t.saturating_duration_since(now));

        let signals = wait_signals(pipe, timeout);
        for sig in signals {
            match sig {
                libc::SIGCHLD => {}
                libc::SIGTERM | libc::SIGINT | libc::SIGQUIT => {
                    terminating = true;
                    match workload_pid {
                        Some(pid) => kill(pid, sig),
                        None if stopping.is_none() => {
                            stopping = Some(begin_stop(
                                workload_code
                                    .filter(|_| cfg.on_exit == OnExit::Exit)
                                    .unwrap_or(0),
                                &cfg,
                                server_pid,
                            ));
                        }
                        None => {}
                    }
                }
                other => {
                    if let Some(pid) = workload_pid {
                        kill(pid, other);
                    }
                }
            }
        }
        // Reap everything that has exited, children and orphans alike.
        loop {
            let mut status: libc::c_int = 0;
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if pid <= 0 {
                break;
            }
            let pid = pid as u32;
            if Some(pid) == workload_pid {
                workload_pid = None;
                let code = decode_status(status);
                workload_code = Some(code);
                if cfg.on_exit == OnExit::Exit || terminating {
                    if stopping.is_none() {
                        stopping = Some(begin_stop(code, &cfg, server_pid));
                    }
                } else {
                    say(format!(
                        "the image's command exited with status {code}; cua-spacesd keeps serving"
                    ));
                }
            } else if Some(pid) == server_pid {
                server_pid = None;
                if stopping.is_none() {
                    // Healthy for a while: start over with a short delay.
                    if server_started.elapsed() > Duration::from_secs(30) {
                        server_failures = 0;
                    }
                    server_failures += 1;
                    say(format!(
                        "cua-spacesd exited with status {}; starting it again",
                        decode_status(status)
                    ));
                    server_restart_at = Some(Instant::now() + backoff(server_failures));
                }
            }
        }
    }
}

/// Ends the Space: stops cua-spacesd (the command has gone).
fn begin_stop(code: i32, cfg: &Config, server_pid: Option<u32>) -> Stopping {
    if let Some(pid) = server_pid {
        kill(pid, libc::SIGTERM);
    }
    Stopping {
        code,
        kill_at: Instant::now() + cfg.stop_grace,
        killed: false,
    }
}

/// 250 ms, 500 ms, ... up to 5 s between starts of a failing cua-spacesd.
pub fn backoff(failures: u32) -> Duration {
    Duration::from_millis(
        250u64
            .saturating_mul(1 << failures.saturating_sub(1).min(5))
            .min(5000),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_restart_backoff_grows_to_five_seconds() {
        let ms: Vec<u128> = (1..=7).map(|n| backoff(n).as_millis()).collect();
        assert_eq!(ms, [250, 500, 1000, 2000, 4000, 5000, 5000]);
    }

    #[test]
    fn wait_statuses_decode_like_a_shell() {
        // exit(7) is 7 << 8; killed by SIGKILL is 9.
        assert_eq!(decode_status(7 << 8), 7);
        assert_eq!(decode_status(libc::SIGKILL), 128 + 9);
        assert_eq!(decode_status(0), 0);
    }
}
