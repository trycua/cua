//! Who is calling the daemon, and what the approval gate does about it.
//!
//! Trust model (the gate lives here, where capabilities execute, so every
//! entry point is covered, not only the MCP front end):
//!
//! - **Verified Cua code is the user.** A peer of the Unix socket that a
//!   [`PeerVerifier`] vouches for (the signed Cua Spaces app, or the signed
//!   `cua` CLI: user commands in a terminal, the app's own UI actions) is
//!   [`Caller::User`] and passes without extra prompts. `cua mcp` is the
//!   signed CLI too, and applies the same gate itself before it calls.
//! - **Everything else is an agent.** Any other process of the user (a raw
//!   socket client, `curl`, a script), a loopback caller holding only the
//!   token in the discovery file, and every request to `/mcp` are
//!   [`Caller::Agent`]: the user's approval policy applies to what they ask
//!   for, and where nothing can ask the user the call fails closed.
//! - **The token is the identity on loopback.** The discovery-file token
//!   (readable by any process of the user) is the agent tier. A second,
//!   random, in-memory token is handed out only in responses to verified
//!   peers, so the env and service passthroughs a verified client uses keep
//!   working without a prompt while a process that merely read the file
//!   cannot get it.
//!
//! Not covered: an agent that runs the signed `cua` CLI in its own shell is
//! the CLI, and so the user, here. Windows has no peer identity on loopback,
//! so only `/mcp` is gated there, and likewise for a daemon with no socket.

use std::sync::Arc;

/// Who is on the other end of a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The user, through verified Cua code.
    User,
    /// Any other caller, by the name of its program; the policy applies.
    Agent(String),
}

impl Caller {
    /// Whether the user's approval policy applies to this caller.
    pub fn is_agent(&self) -> bool {
        matches!(self, Caller::Agent(_))
    }
}

/// Decides who the process on the other end of a connected Unix socket is.
pub trait PeerVerifier: Send + Sync + std::fmt::Debug {
    /// The caller behind the connected socket `fd`. Must never trust what
    /// the peer says: only what the OS reports about it.
    fn verify(&self, fd: i32) -> Caller;
}

/// Trusts every peer: tests and fixtures only.
#[derive(Debug)]
pub struct TrustAll;

impl PeerVerifier for TrustAll {
    fn verify(&self, _fd: i32) -> Caller {
        Caller::User
    }
}

/// The default verifier: a peer of the same user that runs the daemon's own
/// executable (or a program in the daemon's `.app` bundle) is the user's own
/// `cua`; anything else is an agent. A build that can check code signatures
/// supplies a stronger verifier (`ServerConfig::peer_verifier`).
#[derive(Debug)]
pub struct SameProgram {
    own: Option<std::path::PathBuf>,
}

impl SameProgram {
    /// A verifier for the running daemon.
    pub fn new() -> Self {
        SameProgram {
            own: std::env::current_exe()
                .ok()
                .and_then(|p| p.canonicalize().ok()),
        }
    }
}

impl Default for SameProgram {
    fn default() -> Self {
        Self::new()
    }
}

/// The `.app` bundle directory that contains `p`, if any.
fn bundle_of(p: &std::path::Path) -> Option<&std::path::Path> {
    p.ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
}

#[cfg(unix)]
impl PeerVerifier for SameProgram {
    fn verify(&self, fd: i32) -> Caller {
        let Some((pid, uid)) = peer_pid_uid(fd) else {
            return Caller::Agent("unknown".into());
        };
        // SAFETY: geteuid has no preconditions.
        if uid != unsafe { libc::geteuid() } {
            return Caller::Agent("another user".into());
        }
        let Some(exe) = peer_exe(pid) else {
            return Caller::Agent("unknown".into());
        };
        let name = exe
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into());
        let same = self.own.as_ref().is_some_and(|own| {
            *own == exe
                || bundle_of(own)
                    .zip(bundle_of(&exe))
                    .is_some_and(|(a, b)| a == b)
        });
        if same {
            Caller::User
        } else {
            Caller::Agent(name)
        }
    }
}

#[cfg(not(unix))]
impl PeerVerifier for SameProgram {
    fn verify(&self, _fd: i32) -> Caller {
        Caller::User
    }
}

/// Pid and effective uid of the peer of a connected Unix socket.
#[cfg(unix)]
pub fn peer_pid_uid(fd: i32) -> Option<(i32, u32)> {
    #[cfg(target_os = "macos")]
    {
        const SOL_LOCAL: libc::c_int = 0;
        const LOCAL_PEERPID: libc::c_int = 0x002;
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: fd is a connected socket; pid and len are valid for writes.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                SOL_LOCAL,
                LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut len,
            )
        };
        if rc != 0 || pid <= 0 {
            return None;
        }
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: getpeereid writes the peer's ids into the two out-params.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return None;
        }
        Some((pid, uid))
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: fd is a connected socket; cred and len are valid for writes.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        (rc == 0 && cred.pid > 0).then_some((cred.pid, cred.uid))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = fd;
        None
    }
}

/// The executable of process `pid`, symlinks resolved.
#[cfg(unix)]
pub fn peer_exe(pid: i32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: buf is writable for the length given.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        std::path::PathBuf::from(String::from_utf8(buf).ok()?)
            .canonicalize()
            .ok()
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()?
            .canonicalize()
            .ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// The verifier a daemon uses when none is configured.
pub fn default_verifier() -> Arc<dyn PeerVerifier> {
    Arc::new(SameProgram::new())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn a_peer_in_this_process_is_this_program() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        // The test binary is the daemon's executable here.
        assert_eq!(SameProgram::new().verify(a.as_raw_fd()), Caller::User);
    }

    #[test]
    fn another_program_is_an_agent_named_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s");
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        // `nc -U` connects to a Unix socket on macOS and most Linux hosts.
        let mut nc = match std::process::Command::new("nc")
            .arg("-U")
            .arg(&sock)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let (peer, _) = l.accept().unwrap();
        let who = SameProgram::new().verify(peer.as_raw_fd());
        let _ = nc.kill();
        let _ = nc.wait();
        assert!(matches!(&who, Caller::Agent(n) if n == "nc"), "{who:?}");
    }
}
