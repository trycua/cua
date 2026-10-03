// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spawning processes with pipes or a pseudo-terminal.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
#[cfg(not(windows))]
use std::pin::Pin;
#[cfg(not(windows))]
use std::sync::Arc;
#[cfg(not(windows))]
use std::task::{Context, Poll};

#[cfg(not(windows))]
use tokio::io::ReadBuf;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};

/// A resolved OS user.
#[derive(Debug, Clone)]
pub struct UserInfo {
    /// Login name.
    pub name: String,
    /// Home directory.
    pub home: PathBuf,
    /// Login shell.
    pub shell: String,
    /// Numeric uid (Unix).
    #[cfg(unix)]
    pub uid: u32,
    /// Primary gid (Unix).
    #[cfg(unix)]
    pub gid: u32,
}

/// Looks up `name` (Unix: `getpwnam_r`).
#[cfg(unix)]
pub fn lookup_user(name: &str) -> io::Result<UserInfo> {
    use std::ffi::{CStr, CString};
    let cname = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut buf = vec![0u8; 16 * 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: all pointers are valid for the call; `buf` outlives `pwd`'s
    // borrowed strings, which are copied out before returning.
    let rc = unsafe {
        libc::getpwnam_r(
            cname.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc));
    }
    if result.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no such user {name:?}"),
        ));
    }
    // SAFETY: getpwnam_r succeeded, so the string fields point into `buf`.
    let (home, shell) = unsafe {
        (
            CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned(),
            CStr::from_ptr(pwd.pw_shell).to_string_lossy().into_owned(),
        )
    };
    Ok(UserInfo {
        name: name.to_owned(),
        home: PathBuf::from(home),
        shell,
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
    })
}

/// Looks up `name`. Only the current user is supported off Unix.
#[cfg(not(unix))]
pub fn lookup_user(name: &str) -> io::Result<UserInfo> {
    let current = std::env::var("USERNAME").unwrap_or_default();
    if !name.eq_ignore_ascii_case(&current) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "running processes as another user is not supported on this platform",
        ));
    }
    Ok(UserInfo {
        name: current,
        home: crate::config::home_dir().unwrap_or_default(),
        shell: "cmd.exe".into(),
    })
}

/// Name of the user the driver runs as.
pub fn current_user_name() -> String {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: getpwuid returns a pointer to static storage or null.
        unsafe {
            let pwd = libc::getpwuid(libc::geteuid());
            if !pwd.is_null() {
                return CStr::from_ptr((*pwd).pw_name)
                    .to_string_lossy()
                    .into_owned();
            }
        }
        std::env::var("USER").unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERNAME").unwrap_or_default()
    }
}

/// Environment variables never inherited by child processes.
pub const SCRUBBED_ENV: &[&str] = &["CUA_ENV_TOKEN", "CUA_SPACESD_TOKEN", "CUA_RELAY_TOKEN"];

/// Everything needed to spawn.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// Executable.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Final environment (replaces the driver's).
    pub env: BTreeMap<String, String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// User to switch to, when different from the driver's.
    pub switch_user: Option<UserInfo>,
    /// Keep stdin open.
    pub stdin: bool,
    /// Run under a PTY of this size (`cols`, `rows`, `px_w`, `px_h`).
    pub pty: Option<(u16, u16, u16, u16)>,
}

/// A spawned child process.
#[cfg(unix)]
pub type ChildProc = Child;

/// A spawned child process: piped, or under ConPTY on Windows.
#[cfg(not(unix))]
// One per managed process; boxing the tokio child buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum ChildProc {
    /// Pipes mode.
    Pipes(Child),
    /// PTY mode.
    #[cfg(windows)]
    ConPty(super::conpty::ConPtyChild),
}

#[cfg(not(unix))]
impl ChildProc {
    /// Waits for exit (cancel-safe).
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        match self {
            Self::Pipes(child) => child.wait().await,
            #[cfg(windows)]
            Self::ConPty(child) => child.wait().await,
        }
    }

    /// Starts killing the process.
    pub fn start_kill(&mut self) -> io::Result<()> {
        match self {
            Self::Pipes(child) => child.start_kill(),
            #[cfg(windows)]
            Self::ConPty(child) => child.start_kill(),
        }
    }
}

/// A spawned process and its I/O handles.
pub struct Spawned {
    /// The child.
    pub child: ChildProc,
    /// OS pid.
    pub pid: u32,
    /// stdin writer (pipes mode, when requested).
    pub stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    /// stdout reader (pipes mode).
    pub stdout: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// stderr reader (pipes mode).
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// PTY master (PTY mode).
    pub pty: Option<PtyMaster>,
}

fn base_command(spec: &SpawnSpec) -> Command {
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .kill_on_drop(false);
    command
}

/// Spawns with pipes.
pub fn spawn_pipes(spec: &SpawnSpec) -> io::Result<Spawned> {
    use std::process::Stdio;
    let mut command = base_command(spec);
    command
        .stdin(if spec.stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
        install_user_switch(&mut command, spec.switch_user.clone(), false);
    }
    let mut child = command.spawn()?;
    let pid = child.id().unwrap_or(0);
    let stdin = child
        .stdin
        .take()
        .map(|s| Box::new(s) as Box<dyn AsyncWrite + Send + Unpin>);
    let stdout = child
        .stdout
        .take()
        .map(|s| Box::new(s) as Box<dyn AsyncRead + Send + Unpin>);
    let stderr = child
        .stderr
        .take()
        .map(|s| Box::new(s) as Box<dyn AsyncRead + Send + Unpin>);
    #[cfg(not(unix))]
    let child = ChildProc::Pipes(child);
    Ok(Spawned {
        child,
        pid,
        stdin,
        stdout,
        stderr,
        pty: None,
    })
}

/// Runs in the forked child before `exec`: new session, controlling
/// terminal, then the user switch.
#[cfg(unix)]
fn install_user_switch(command: &mut Command, user: Option<UserInfo>, pty: bool) {
    use std::ffi::CString;
    let groups_name = user
        .as_ref()
        .and_then(|u| CString::new(u.name.clone()).ok());
    // SAFETY: only async-signal-safe libc calls run between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if pty {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if let Some(user) = &user {
                if let Some(name) = &groups_name {
                    if libc::initgroups(name.as_ptr(), user.gid as _) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                if libc::setgid(user.gid) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::setuid(user.uid) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

/// Spawns under a new pseudo-terminal.
#[cfg(unix)]
pub fn spawn_pty(spec: &SpawnSpec) -> io::Result<Spawned> {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::process::Stdio;

    let (cols, rows, px_w, px_h) = spec.pty.unwrap_or((80, 24, 0, 0));
    let mut size = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: px_w,
        ws_ypixel: px_h,
    };
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // SAFETY: out-pointers are valid; name and termios may be null.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two fresh descriptors we now own.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    set_cloexec(&master)?;
    set_cloexec(&slave)?;

    let mut command = base_command(spec);
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    install_user_switch(&mut command, spec.switch_user.clone(), true);
    let child = command.spawn()?;
    // `command` still holds the slave descriptors; drop it so only the child
    // keeps the terminal open and the master sees EOF when the child exits.
    drop(command);
    let pid = child.id().unwrap_or(0);
    let master = PtyMaster::new(master)?;
    Ok(Spawned {
        child,
        pid,
        stdin: None,
        stdout: None,
        stderr: None,
        pty: Some(master),
    })
}

/// Spawns under a new pseudo console (ConPTY).
#[cfg(windows)]
pub fn spawn_pty(spec: &SpawnSpec) -> io::Result<Spawned> {
    let (child, pid, master) = super::conpty::spawn(spec)?;
    Ok(Spawned {
        child: ChildProc::ConPty(child),
        pid,
        stdin: None,
        stdout: None,
        stderr: None,
        pty: Some(master),
    })
}

/// PTYs are not supported on this platform.
#[cfg(not(any(unix, windows)))]
pub fn spawn_pty(_spec: &SpawnSpec) -> io::Result<Spawned> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "PTY processes are not supported on this platform",
    ))
}

#[cfg(unix)]
fn set_cloexec(fd: &std::os::fd::OwnedFd) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: fcntl on a valid descriptor.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
pub use super::conpty::PtyMaster;

/// Non-blocking PTY master shared by the reader and writer halves.
#[cfg(not(windows))]
#[derive(Clone)]
pub struct PtyMaster {
    #[cfg(unix)]
    fd: Arc<tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>>,
    #[cfg(not(unix))]
    _never: Arc<()>,
}

#[cfg(unix)]
impl PtyMaster {
    fn new(fd: std::os::fd::OwnedFd) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        // SAFETY: fcntl on a valid descriptor.
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            fd: Arc::new(tokio::io::unix::AsyncFd::new(fd)?),
        })
    }

    /// Sets the terminal size (the kernel delivers SIGWINCH).
    pub fn resize(&self, cols: u16, rows: u16, px_w: u16, px_h: u16) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let size = libc::winsize {
            ws_row: rows.max(1),
            ws_col: cols.max(1),
            ws_xpixel: px_w,
            ws_ypixel: px_h,
        };
        // SAFETY: TIOCSWINSZ with a valid winsize pointer.
        let rc = unsafe { libc::ioctl(self.fd.get_ref().as_raw_fd(), libc::TIOCSWINSZ, &size) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
impl PtyMaster {
    /// Unreachable off Unix.
    pub fn resize(&self, _cols: u16, _rows: u16, _px_w: u16, _px_h: u16) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(unix)]
impl AsyncRead for PtyMaster {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        use std::os::fd::AsRawFd;
        loop {
            let mut guard = match self.fd.poll_read_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            let unfilled = buf.initialize_unfilled();
            // SAFETY: reading into an initialized, exclusively borrowed slice.
            let n = unsafe {
                libc::read(
                    guard.get_inner().as_raw_fd(),
                    unfilled.as_mut_ptr().cast(),
                    unfilled.len(),
                )
            };
            if n >= 0 {
                buf.advance(n as usize);
                return Poll::Ready(Ok(()));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EAGAIN) => {
                    guard.clear_ready();
                    continue;
                }
                Some(libc::EINTR) => continue,
                // Linux reports EIO once every slave descriptor is closed.
                Some(libc::EIO) => return Poll::Ready(Ok(())),
                _ => return Poll::Ready(Err(error)),
            }
        }
    }
}

#[cfg(unix)]
impl AsyncWrite for PtyMaster {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        use std::os::fd::AsRawFd;
        loop {
            let mut guard = match self.fd.poll_write_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            // SAFETY: writing from a valid slice.
            let n = unsafe {
                libc::write(
                    guard.get_inner().as_raw_fd(),
                    data.as_ptr().cast(),
                    data.len(),
                )
            };
            if n >= 0 {
                return Poll::Ready(Ok(n as usize));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EAGAIN) => {
                    guard.clear_ready();
                    continue;
                }
                Some(libc::EINTR) => continue,
                _ => return Poll::Ready(Err(error)),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(not(any(unix, windows)))]
impl AsyncRead for PtyMaster {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::from(io::ErrorKind::Unsupported)))
    }
}

#[cfg(not(any(unix, windows)))]
impl AsyncWrite for PtyMaster {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::from(io::ErrorKind::Unsupported)))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Maps a contract signal to the native number (Unix).
#[cfg(unix)]
pub fn native_signal(signal: cua_proto::env::v1::Signal) -> Option<libc::c_int> {
    use cua_proto::env::v1::Signal as S;
    Some(match signal {
        S::Hup => libc::SIGHUP,
        S::Int => libc::SIGINT,
        S::Quit => libc::SIGQUIT,
        S::Kill => libc::SIGKILL,
        S::Usr1 => libc::SIGUSR1,
        S::Usr2 => libc::SIGUSR2,
        S::Term => libc::SIGTERM,
        S::Cont => libc::SIGCONT,
        S::Stop => libc::SIGSTOP,
        S::Tstp => libc::SIGTSTP,
        S::Winch => libc::SIGWINCH,
        S::Unspecified => return None,
    })
}

/// Maps a native signal number back to the contract enum (Unix).
#[cfg(unix)]
pub fn contract_signal(native: libc::c_int) -> cua_proto::env::v1::Signal {
    use cua_proto::env::v1::Signal as S;
    match native {
        libc::SIGHUP => S::Hup,
        libc::SIGINT => S::Int,
        libc::SIGQUIT => S::Quit,
        libc::SIGKILL => S::Kill,
        libc::SIGUSR1 => S::Usr1,
        libc::SIGUSR2 => S::Usr2,
        libc::SIGTERM => S::Term,
        libc::SIGCONT => S::Cont,
        libc::SIGSTOP => S::Stop,
        libc::SIGTSTP => S::Tstp,
        libc::SIGWINCH => S::Winch,
        _ => S::Unspecified,
    }
}
